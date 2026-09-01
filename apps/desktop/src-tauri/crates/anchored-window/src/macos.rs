use std::sync::{Arc, Mutex};

use objc2_app_kit::{NSEvent, NSWindow};
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{Manager, Runtime, WebviewWindow, WebviewWindowBuilder};

use crate::geometry::{Rect, place_left_preferred};
use crate::{AnchorRegion, WindowMaterial};

pub(crate) struct FrameRequest {
    pub width: f64,
    pub height: Option<f64>,
    pub anchor_region: AnchorRegion,
    pub gap: f64,
    pub screen_margin: f64,
}

pub(crate) fn configure<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
    material: WindowMaterial,
) -> WebviewWindowBuilder<'a, R, M> {
    let builder = builder.accept_first_mouse(true);
    match material {
        WindowMaterial::Popover { corner_radius } => builder.effects(
            EffectsBuilder::new()
                .effect(Effect::Popover)
                .state(EffectState::Active)
                .radius(corner_radius)
                .build(),
        ),
        WindowMaterial::Opaque | WindowMaterial::Transparent => builder,
    }
}

pub(crate) fn show_without_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let window = window.clone();
    window.clone().run_on_main_thread(move || {
        if let Ok(pointer) = window.ns_window() {
            // SAFETY: The pointer is the live NSWindow, and this code runs on the main thread.
            unsafe {
                (&*pointer.cast::<NSWindow>()).orderFrontRegardless();
            }
        } else {
            tracing::warn!("failed to access the anchored NSWindow");
        }
    })
}

pub(crate) fn apply_frame(
    anchor: &WebviewWindow,
    companion: &WebviewWindow,
    request: FrameRequest,
    native_frame: Arc<Mutex<Option<Rect>>>,
) -> tauri::Result<()> {
    let anchor = anchor.clone();
    let companion = companion.clone();
    companion.clone().run_on_main_thread(move || {
        apply_frame_on_main_thread(&anchor, &companion, request, &native_frame);
    })
}

fn apply_frame_on_main_thread(
    anchor: &WebviewWindow,
    companion: &WebviewWindow,
    request: FrameRequest,
    native_frame: &Mutex<Option<Rect>>,
) {
    let (Ok(anchor_pointer), Ok(companion_pointer)) = (anchor.ns_window(), companion.ns_window())
    else {
        tracing::warn!("failed to access anchored NSWindow values");
        return;
    };
    // SAFETY: The pointers are live NSWindow values, and this code runs on the main thread.
    unsafe {
        update_frame(
            &*anchor_pointer.cast::<NSWindow>(),
            &*companion_pointer.cast::<NSWindow>(),
            request,
            native_frame,
        );
    }
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
    let height = request.height.unwrap_or(anchor_frame.size.height);
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

pub(crate) fn cursor_inside(native_frame: &Mutex<Option<Rect>>) -> bool {
    let Ok(cached) = native_frame.lock() else {
        tracing::warn!("failed to read the anchored native frame");
        return false;
    };
    let Some(frame) = *cached else {
        return false;
    };
    let cursor = NSEvent::mouseLocation();
    frame_contains(frame, cursor.x, cursor.y)
}

fn frame_contains(frame: Rect, x: f64, y: f64) -> bool {
    x >= frame.x && x < frame.x + frame.width && y >= frame.y && y < frame.y + frame.height
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

pub(crate) fn hide(window: &WebviewWindow) -> tauri::Result<()> {
    window.hide()
}

#[cfg(test)]
mod tests;
