// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Text drawn next to the tray glyph in the menu bar, as an attributed title on
//! the status button.
//!
//! **Why not `TrayIcon::set_title`.** Tauri's tray exposes a `set_title`, and it
//! looks like the answer. It isn't, for two reasons. It calls `-[NSButton
//! setTitle:]` with a plain string, so there is no control over the font or where
//! the glyphs sit vertically; and passing `None` is a no-op — tray-icon's
//! `set_title_inner` is an `if let Some(title)` with no `else` branch, so once a
//! title is on screen there is no way to take it off. A title that can't be
//! cleared can't implement "only visible when low".
//!
//! So this module sets an **`NSAttributedString`** directly, which buys the three
//! things a plain title can't give:
//!
//! - `monospacedDigitSystemFont`, so the number doesn't shift sideways as digits
//!   change width when free space ticks from 49 to 48.
//! - A **baseline offset**, which is what corrects the too-high look that plain
//!   menu-bar text has (see [`BASELINE_OFFSET`]).
//! - `labelColor`, so the text dims and inverts with the menu bar exactly like
//!   the template glyph beside it. A hardcoded white or black is wrong in at
//!   least one of light mode, dark mode, or a menu bar tinted by the wallpaper.
//!
//! This is mechanism only, macOS-only, a no-op elsewhere, and stateless (each
//! call fully describes the desired title). All AppKit work happens inside
//! `with_inner_tray_icon` so it runs on the main thread.
//!
//! Bypassing tray-icon has one consequence this module has to clean up itself —
//! see [`imp::sync_click_target`].

/// Show `title` next to the tray glyph, or remove it when `None`.
///
/// Idempotent and stateless. macOS-only; a no-op elsewhere.
#[cfg(target_os = "macos")]
pub fn set_tray_title<R: tauri::Runtime>(tray: &tauri::tray::TrayIcon<R>, title: Option<String>) {
    imp::set_tray_title(tray, title);
}

#[cfg(not(target_os = "macos"))]
pub fn set_tray_title<R: tauri::Runtime>(_tray: &tauri::tray::TrayIcon<R>, _title: Option<String>) {
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSBaselineOffsetAttributeName, NSColor, NSFont, NSFontAttributeName, NSFontWeightRegular,
        NSForegroundColorAttributeName, NSStatusBarButton,
    };
    use objc2_foundation::{
        NSAttributedString, NSAttributedStringKey, NSDictionary, NSNumber, NSString,
    };
    use tauri::Runtime;

    /// How far to drop the text from where AppKit would otherwise put it, in
    /// points.
    ///
    /// Menu-bar text drawn the obvious way sits fractionally too high — visible
    /// in any menu-bar app that draws a SwiftUI `Text` in a `MenuBarExtra`. The
    /// cause is that the text is centered on the font's *line box*, and a line
    /// box reserves room for descenders (the tails on g, y, p). "48 GB" has no
    /// descenders, so that reserved space sits empty below the digits and pushes
    /// the visible glyphs above the optical centre of the 22pt menu bar. It is
    /// off by about a point: not enough to name, enough to notice.
    ///
    /// SwiftUI offers no lever for this; an attributed string does. Tuned
    /// against the neighbouring native menu-bar items — adjust here if a future
    /// macOS changes the menu-bar metrics.
    const BASELINE_OFFSET: f64 = -1.0;

    /// Gap between the glyph and the first digit, as a leading hair space.
    /// AppKit's own image-then-title spacing is tighter than the menu bar's
    /// optical rhythm, which makes the number read as attached to the logo.
    const TITLE_LEADING_SPACE: &str = "\u{2009}";

    pub fn set_tray_title<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, title: Option<String>) {
        // The closure runs on the main thread (Tauri dispatches it there), so the
        // AppKit objects we touch never cross a thread boundary.
        let _ = tray.with_inner_tray_icon(move |inner| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let Some(item) = inner.ns_status_item() else {
                return;
            };
            let Some(button) = item.button(mtm) else {
                return;
            };

            // An empty attributed string is how the title is *cleared*: AppKit
            // collapses the title region to zero width and the variable-length
            // status item shrinks back to glyph-only.
            let attributed = match title.as_deref() {
                Some(text) if !text.is_empty() => {
                    styled_title(&format!("{TITLE_LEADING_SPACE}{text}"))
                }
                _ => NSAttributedString::new(),
            };
            button.setAttributedTitle(&attributed);

            sync_click_target(&button);
        });
    }

    /// Build the menu-bar title: monospaced digits at the menu-bar font size,
    /// `labelColor` so it tracks the menu bar's appearance, and the baseline
    /// correction from [`BASELINE_OFFSET`].
    fn styled_title(text: &str) -> Retained<NSAttributedString> {
        // `menuBarFontOfSize(0.0)` means "the default menu-bar size", which is
        // where the size has to come from — it is not the same as the system
        // font size, and it follows the user's menu-bar sizing.
        let point_size = NSFont::menuBarFontOfSize(0.0).pointSize();
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(point_size, unsafe {
            NSFontWeightRegular
        });
        let color = NSColor::labelColor();
        let baseline = NSNumber::numberWithDouble(BASELINE_OFFSET);

        let keys: [&NSAttributedStringKey; 3] = unsafe {
            [
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSBaselineOffsetAttributeName,
            ]
        };
        // Bound above rather than inline in the array: these are `Retained<_>`
        // temporaries, and the coercion to `&AnyObject` would otherwise drop
        // them at the end of the expression.
        let values: [&AnyObject; 3] = [&font, &color, &baseline];
        let attributes = NSDictionary::from_slices(&keys, &values);

        unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attributes),
            )
        }
    }

    /// Resize the status button's subviews to the button, because we changed the
    /// button's width behind tray-icon's back.
    ///
    /// tray-icon lays an invisible `TrayTarget` view over the button to catch
    /// clicks, and re-syncs its frame *only* inside its own `set_icon` /
    /// `set_title` / `set_tooltip` (`TrayTarget::update_dimensions`). Setting the
    /// title ourselves means AppKit widens the variable-length status item and
    /// nothing widens the click catcher — so the part of the item showing the
    /// number would be visually present but dead to clicks. This does what
    /// `update_dimensions` does, at the same moment.
    ///
    /// Every subview is resized rather than the click catcher specifically: an
    /// `NSStatusBarButton` draws its image and title through its cell, not
    /// through subviews, so tray-icon's target is the only subview there is, and
    /// matching on a private type name would be the more fragile coupling.
    fn sync_click_target(button: &NSStatusBarButton) {
        let frame = button.frame();
        for subview in button.subviews() {
            subview.setFrame(frame);
        }
    }
}
