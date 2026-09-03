use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use gtk::gdk::prelude::DisplayExtManual;
use gtk::gdk::{CrossingMode, EventMask, NotifyType};
use gtk::prelude::*;
use tauri::{Manager, Runtime, WebviewWindow, WebviewWindowBuilder};

use crate::geometry::CursorProximity;

const POINTER_PENDING: u8 = 0;
const POINTER_INSTALLING: u8 = 1;
const POINTER_GLOBAL: u8 = 2;
const POINTER_OUTSIDE: u8 = 3;
const POINTER_INSIDE: u8 = 4;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CursorSource {
    Pending,
    Global,
    Local(CursorProximity),
}

pub(crate) struct PointerTracker {
    state: AtomicU8,
}

impl PointerTracker {
    pub(crate) fn new() -> Self {
        Self {
            state: AtomicU8::new(POINTER_PENDING),
        }
    }

    fn begin_install(&self) -> bool {
        self.state
            .compare_exchange(
                POINTER_PENDING,
                POINTER_INSTALLING,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    fn set(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
    }

    pub(crate) fn reset_for_show(&self) {
        if self.state.load(Ordering::Relaxed) >= POINTER_OUTSIDE {
            self.set(POINTER_OUTSIDE);
        }
    }

    pub(crate) fn reset_for_install(&self) {
        self.set(POINTER_PENDING);
    }

    pub(crate) fn source(&self) -> CursorSource {
        match self.state.load(Ordering::Relaxed) {
            POINTER_GLOBAL => CursorSource::Global,
            POINTER_OUTSIDE => CursorSource::Local(CursorProximity::Outside),
            POINTER_INSIDE => CursorSource::Local(CursorProximity::Inside),
            _ => CursorSource::Pending,
        }
    }
}

pub(crate) fn configure<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

pub(crate) fn install_pointer_tracking(
    window: &WebviewWindow,
    tracker: Arc<PointerTracker>,
) -> tauri::Result<()> {
    if !tracker.begin_install() {
        return Ok(());
    }
    let window = window.clone();
    window.clone().run_on_main_thread(move || {
        let Ok(gtk_window) = window.gtk_window() else {
            tracker.set(POINTER_PENDING);
            tracing::warn!("failed to access the anchored GTK window for pointer tracking");
            return;
        };
        if !gtk_window.display().backend().is_wayland() {
            tracker.set(POINTER_GLOBAL);
            return;
        }

        // Wayland does not expose global pointer coordinates. Use exact window crossing events with the configured exit delay.
        tracker.set(POINTER_OUTSIDE);
        gtk_window.add_events(EventMask::ENTER_NOTIFY_MASK | EventMask::LEAVE_NOTIFY_MASK);
        let enter_tracker = Arc::clone(&tracker);
        gtk_window.connect_enter_notify_event(move |_, event| {
            if is_window_crossing(event) {
                enter_tracker.set(POINTER_INSIDE);
            }
            gtk::glib::Propagation::Proceed
        });
        gtk_window.connect_leave_notify_event(move |_, event| {
            if is_window_crossing(event) {
                tracker.set(POINTER_OUTSIDE);
            }
            gtk::glib::Propagation::Proceed
        });
    })
}

fn is_window_crossing(event: &gtk::gdk::EventCrossing) -> bool {
    is_window_crossing_kind(event.mode(), event.detail())
}

fn is_window_crossing_kind(mode: CrossingMode, detail: NotifyType) -> bool {
    mode == CrossingMode::Normal && detail != NotifyType::Inferior
}

pub(crate) fn show_without_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let window = window.clone();
    window.clone().run_on_main_thread(move || {
        let Ok(gtk_window) = window.gtk_window() else {
            tracing::warn!("failed to access the anchored GTK window");
            return;
        };
        gtk_window.set_focus_on_map(false);
        if let Err(error) = window.show() {
            tracing::warn!(%error, "failed to show the anchored GTK window");
        }
    })
}

pub(crate) fn hide(window: &WebviewWindow) -> tauri::Result<()> {
    window.hide()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_tracker_resets_reveal_and_reinstallation_state() {
        let tracker = PointerTracker::new();
        assert_eq!(tracker.source(), CursorSource::Pending);

        tracker.set(POINTER_INSIDE);
        tracker.reset_for_show();
        assert_eq!(
            tracker.source(),
            CursorSource::Local(CursorProximity::Outside)
        );

        tracker.set(POINTER_GLOBAL);
        tracker.reset_for_show();
        assert_eq!(tracker.source(), CursorSource::Global);

        tracker.reset_for_install();
        assert_eq!(tracker.source(), CursorSource::Pending);
    }

    #[test]
    fn pointer_tracker_uses_only_physical_window_crossings() {
        assert!(is_window_crossing_kind(
            CrossingMode::Normal,
            NotifyType::Ancestor
        ));
        assert!(!is_window_crossing_kind(
            CrossingMode::Normal,
            NotifyType::Inferior
        ));
        assert!(!is_window_crossing_kind(
            CrossingMode::Grab,
            NotifyType::Ancestor
        ));
    }
}
