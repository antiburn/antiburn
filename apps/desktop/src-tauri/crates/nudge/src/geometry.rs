// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Pure display geometry for notification placement — no OS calls, no `cfg`, so
//! every decision the placement code makes is unit-testable on any platform.
//!
//! All rects and points here are **logical**: points, y increasing downward,
//! origin at the top-left of the main display. On macOS that is exactly Core
//! Graphics' global display space, which is also exactly what Tauri's
//! `LogicalPosition` means (tao converts it to AppKit coordinates by flipping y
//! about `CGDisplayPixelsHigh(main)`).
//!
//! Keeping one coordinate space end to end is deliberate. On macOS (and Linux)
//! tao derives each monitor's *physical* rect by scaling its logical geometry by
//! that monitor's own factor, so on a mixed-DPI setup the resulting rects overlap
//! and a point can fall inside two of them — they cannot be hit-tested reliably.
//! (Windows is exempt: there tao reports true Win32 physical rects, which do not
//! overlap.)

/// A display (or window) rect in logical points, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A point in logical points, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

impl Rect {
    /// Half-open on both axes, so displays that abut share no point and a point
    /// on the seam belongs to exactly one of them.
    pub(crate) fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.w && p.y >= self.y && p.y < self.y + self.h
    }
}

/// Index of the first display containing `p`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn display_containing(displays: &[Rect], p: Point) -> Option<usize> {
    displays.iter().position(|d| d.contains(p))
}

/// Which signal picked the display. Returned alongside the index and logged, so
/// a notification that lands on the wrong screen can be diagnosed from a user's
/// log file instead of guessed at.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplaySource {
    FrontmostWindow,
    Cursor,
    MainDisplay,
}

/// Pick the display a notification should appear on, in priority order:
///
/// 1. the display showing the frontmost application's front window,
/// 2. the display under the cursor — used when the frontmost application is
///    antiburn itself (a tray click or a dev-sim), which has no window that tells
///    us where the user is working,
/// 3. the main display.
///
/// Tier 1 beats the cursor because it is common to type on one monitor while the
/// mouse rests on another.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn resolve_display(
    displays: &[Rect],
    frontmost_center: Option<Point>,
    cursor: Option<Point>,
    main_index: Option<usize>,
) -> Option<(usize, DisplaySource)> {
    if let Some(i) = frontmost_center.and_then(|p| display_containing(displays, p)) {
        return Some((i, DisplaySource::FrontmostWindow));
    }
    if let Some(i) = cursor.and_then(|p| display_containing(displays, p)) {
        return Some((i, DisplaySource::Cursor));
    }
    main_index.map(|i| (i, DisplaySource::MainDisplay))
}

/// Top-left corner of a notification pinned to the top-right of `display`,
/// inset by `margin` from the right edge and `top_inset` from the top.
///
/// Clamped to the display's left edge so a display narrower than the
/// notification cannot push it off-screen (or onto a neighbouring monitor).
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) fn top_right_origin(display: Rect, w: f64, margin: f64, top_inset: f64) -> (f64, f64) {
    let x = (display.x + display.w - w - margin).max(display.x);
    (x, display.y + top_inset)
}

/// Top-left corner of a notification pinned to the bottom-right of `display`,
/// inset by `margin` from both edges. Clamped to the display's top-left, so a
/// notification taller or wider than the display stays anchored on-screen and
/// overflows off the far edge rather than off the near one.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn bottom_right_origin(display: Rect, w: f64, h: f64, margin: f64) -> (f64, f64) {
    let x = (display.x + display.w - w - margin).max(display.x);
    let y = (display.y + display.h - h - margin).max(display.y);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: f64 = 344.0;
    const MARGIN: f64 = 12.0;
    const TOP_INSET: f64 = 48.0;

    /// A real mixed-DPI arrangement: a 2x built-in beside a 1x external whose top
    /// edge sits *above* it, so the external's origin has a negative y.
    fn displays() -> Vec<Rect> {
        vec![
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1512.0,
                h: 982.0,
            },
            Rect {
                x: 1512.0,
                y: -525.0,
                w: 2560.0,
                h: 1440.0,
            },
        ]
    }

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn display_containing_finds_the_built_in() {
        assert_eq!(display_containing(&displays(), p(200.0, 100.0)), Some(0));
    }

    #[test]
    fn display_containing_handles_a_negative_origin_display() {
        assert_eq!(display_containing(&displays(), p(2000.0, -400.0)), Some(1));
    }

    #[test]
    fn display_containing_is_half_open_at_the_seam() {
        // x = 1512 is the built-in's exclusive right edge and the external's
        // inclusive left edge, so the seam belongs to exactly one display.
        assert_eq!(display_containing(&displays(), p(1511.0, 0.0)), Some(0));
        assert_eq!(display_containing(&displays(), p(1512.0, 0.0)), Some(1));
    }

    #[test]
    fn display_containing_rejects_a_point_off_every_display() {
        assert_eq!(display_containing(&displays(), p(-5.0, -5.0)), None);
    }

    /// The macOS hover watcher (`macos::start_hover_watch`) decides "the cursor
    /// is on the notification" with exactly this hit-test, against the frame
    /// `window::set_frame` recorded — so the notification's own edges, not just
    /// display seams, depend on it being half-open.
    #[test]
    fn notification_frame_hit_test_is_half_open_at_its_edges() {
        // A menu-bar-anchored notification: 344x168 at (1156, 36).
        let notification = Rect {
            x: 1156.0,
            y: 36.0,
            w: 344.0,
            h: 168.0,
        };

        assert!(notification.contains(p(1300.0, 100.0)), "centre");
        assert!(
            notification.contains(p(1156.0, 36.0)),
            "top-left is inclusive"
        );
        assert!(
            !notification.contains(p(1500.0, 100.0)),
            "the right edge is exclusive"
        );
        assert!(
            !notification.contains(p(1300.0, 204.0)),
            "the bottom edge is exclusive"
        );
        assert!(
            !notification.contains(p(1155.0, 100.0)),
            "one point left of the frame"
        );
        assert!(
            !notification.contains(p(1300.0, 35.0)),
            "one point above the frame"
        );
    }

    /// Hover expansion grows the notification downward (`macos::animate_resize`
    /// pins the top edge), so a cursor resting in the region the expansion just
    /// revealed must still read as inside — otherwise hover would flap between
    /// expanded and collapsed.
    #[test]
    fn an_expanded_notification_contains_the_region_it_grew_into() {
        let collapsed = Rect {
            x: 1156.0,
            y: 36.0,
            w: 344.0,
            h: 96.0,
        };
        let expanded = Rect {
            h: 168.0,
            ..collapsed
        };
        let over_the_action_bar = p(1300.0, 150.0);

        assert!(!collapsed.contains(over_the_action_bar));
        assert!(expanded.contains(over_the_action_bar));
    }

    /// The reason this module exists. tao's *physical* rects for the same two
    /// displays are `x∈[0,3024) y∈[0,1964)` and `x∈[1512,4072) y∈[-525,915)` —
    /// they overlap, so a point can be "on" both. In logical points they cannot.
    #[test]
    fn logical_display_rects_never_overlap() {
        let d = displays();
        let (a, b) = (d[0], d[1]);
        let x_overlap = a.x < b.x + b.w && b.x < a.x + a.w;
        let y_overlap = a.y < b.y + b.h && b.y < a.y + a.h;
        assert!(!(x_overlap && y_overlap), "{a:?} overlaps {b:?}");
    }

    #[test]
    fn top_right_origin_on_a_negative_origin_display() {
        let external = displays()[1];
        assert_eq!(
            top_right_origin(external, WIDTH, MARGIN, TOP_INSET),
            (3716.0, -477.0)
        );
    }

    #[test]
    fn top_right_origin_stays_in_points_on_a_retina_display() {
        let built_in = displays()[0];
        assert_eq!(
            top_right_origin(built_in, WIDTH, MARGIN, TOP_INSET),
            (1156.0, 48.0)
        );
    }

    #[test]
    fn bottom_right_origin_insets_from_both_edges() {
        let built_in = displays()[0];
        assert_eq!(
            bottom_right_origin(built_in, WIDTH, 168.0, MARGIN),
            (1156.0, 802.0)
        );
    }

    /// The Windows corner. Anchored on a secondary display so both origin terms
    /// are non-zero — on the primary display they are 0 and a dropped `display.x`
    /// / `display.y` would go unnoticed.
    #[test]
    fn bottom_right_origin_on_a_secondary_display() {
        let external = displays()[1];
        assert_eq!(
            bottom_right_origin(external, WIDTH, 168.0, MARGIN),
            (3716.0, 735.0)
        );
    }

    /// A display too narrow for the notification must not push it off the left
    /// edge (and onto whatever monitor happens to sit there).
    #[test]
    fn corner_origins_clamp_to_a_display_narrower_than_the_notification() {
        let narrow = Rect {
            x: 1512.0,
            y: 0.0,
            w: 300.0,
            h: 240.0,
        };
        assert_eq!(top_right_origin(narrow, WIDTH, MARGIN, TOP_INSET).0, 1512.0);
        assert_eq!(bottom_right_origin(narrow, WIDTH, 168.0, MARGIN).0, 1512.0);
    }

    /// Likewise a notification taller than the display keeps its top edge on
    /// screen rather than sliding off the top.
    #[test]
    fn bottom_right_origin_clamps_a_notification_taller_than_the_display() {
        let shallow = Rect {
            x: 0.0,
            y: 0.0,
            w: 1512.0,
            h: 200.0,
        };
        assert_eq!(bottom_right_origin(shallow, WIDTH, 400.0, MARGIN).1, 0.0);
    }

    #[test]
    fn resolve_prefers_the_frontmost_window_over_an_idle_cursor() {
        // Typing on the external while the mouse rests on the built-in.
        let got = resolve_display(
            &displays(),
            Some(p(2792.0, 210.0)),
            Some(p(700.0, 500.0)),
            Some(0),
        );
        assert_eq!(got, Some((1, DisplaySource::FrontmostWindow)));
    }

    #[test]
    fn resolve_falls_back_to_the_cursor_when_the_app_is_frontmost() {
        let got = resolve_display(&displays(), None, Some(p(2792.0, 210.0)), Some(0));
        assert_eq!(got, Some((1, DisplaySource::Cursor)));
    }

    #[test]
    fn resolve_falls_back_to_the_cursor_when_the_frontmost_window_is_off_screen() {
        let got = resolve_display(
            &displays(),
            Some(p(-9000.0, -9000.0)),
            Some(p(700.0, 500.0)),
            Some(1),
        );
        assert_eq!(got, Some((0, DisplaySource::Cursor)));
    }

    #[test]
    fn resolve_falls_back_to_the_main_display() {
        let got = resolve_display(&displays(), None, None, Some(0));
        assert_eq!(got, Some((0, DisplaySource::MainDisplay)));
    }

    #[test]
    fn resolve_gives_up_without_a_main_display() {
        assert_eq!(resolve_display(&displays(), None, None, None), None);
    }
}
