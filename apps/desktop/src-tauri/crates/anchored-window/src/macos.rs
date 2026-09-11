mod bridge;
mod native;
pub use native::NativeRequestHandler;
pub(crate) use native::NativeWindow;

use crate::AnchorRegion;
use crate::geometry::{CursorProximity, Point, Rect, classify_cursor, place_left_preferred};
use objc2_app_kit::{NSEvent, NSWindow};
use std::sync::{Arc, Mutex};
use tauri::WebviewWindow;

pub(crate) struct FrameRequest {
    pub width: f64,
    pub height: f64,
    pub anchor_region: AnchorRegion,
    pub gap: f64,
    pub screen_margin: f64,
}

pub(crate) fn apply_frame(
    anchor: &WebviewWindow,
    companion: &NativeWindow,
    request: FrameRequest,
    native_frame: Arc<Mutex<Option<Rect>>>,
) -> tauri::Result<()> {
    let anchor = anchor.clone();
    companion.with_objects(move |objects| {
        let Ok(pointer) = anchor.ns_window() else {
            return;
        };
        // SAFETY: The anchor owns this window, and this callback runs on the main thread.
        let anchor = unsafe { &*pointer.cast::<NSWindow>() };
        objects.panel.setLevel(anchor.level());
        update_frame(anchor, &objects.panel, request, &native_frame);
    })
}

fn update_frame(
    anchor_window: &NSWindow,
    companion_window: &NSWindow,
    request: FrameRequest,
    native_frame: &Mutex<Option<Rect>>,
) {
    let anchor_frame = anchor_window.frame();
    let Some(screen) = anchor_window.screen() else {
        tracing::warn!("the anchor window has no screen");
        return;
    };
    let work_frame = screen.visibleFrame();
    let height = request.height;
    let x = horizontal_origin(
        frame_rect(
            anchor_frame.origin.x,
            anchor_frame.size.width,
            anchor_frame.size.height,
        ),
        frame_rect(
            work_frame.origin.x,
            work_frame.size.width,
            work_frame.size.height,
        ),
        height,
        &request,
    );
    let y = vertical_origin(
        anchor_frame.origin.y + anchor_frame.size.height,
        request.anchor_region.top_within(anchor_frame.size.height),
        height,
        work_frame.origin.y,
        work_frame.origin.y + work_frame.size.height,
        request.screen_margin,
    );
    let mut frame = companion_window.frame();
    frame.origin.x = x;
    frame.origin.y = y;
    frame.size.width = request.width;
    frame.size.height = height;
    companion_window.setFrame_display(frame, true);
    cache_frame(
        native_frame,
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    );
}

fn horizontal_origin(
    anchor: Rect,
    work_area: Rect,
    companion_height: f64,
    request: &FrameRequest,
) -> f64 {
    place_left_preferred(
        anchor,
        work_area,
        request.width,
        companion_height,
        1.0,
        request.gap,
        request.screen_margin,
    )
    .x
}

fn frame_rect(x: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y: 0.0,
        width,
        height,
    }
}

fn cache_frame(native_frame: &Mutex<Option<Rect>>, x: f64, y: f64, width: f64, height: f64) {
    let Ok(mut cached) = native_frame.lock() else {
        tracing::warn!("failed to cache the anchored native frame");
        return;
    };
    *cached = Some(Rect {
        x,
        y,
        width,
        height,
    });
}

pub(crate) fn cursor_location(
    native_frame: &Mutex<Option<Rect>>,
    edge_tolerance: f64,
) -> Option<CursorProximity> {
    let Ok(cached) = native_frame.lock() else {
        return None;
    };
    let Some(frame) = *cached else {
        return Some(CursorProximity::Outside);
    };
    let cursor = NSEvent::mouseLocation();
    Some(classify_cursor(
        frame,
        Point {
            x: cursor.x,
            y: cursor.y,
        },
        edge_tolerance,
        1.0,
    ))
}

fn vertical_origin(
    anchor_top: f64,
    target_top: f64,
    height: f64,
    work_bottom: f64,
    work_top: f64,
    screen_margin: f64,
) -> f64 {
    clamp_origin(
        anchor_top - target_top - height,
        work_bottom + screen_margin,
        work_top - screen_margin - height,
    )
}

fn clamp_origin(value: f64, minimum: f64, maximum: f64) -> f64 {
    if maximum < minimum {
        return maximum;
    }
    value.clamp(minimum, maximum)
}

#[cfg(test)]
mod tests;
