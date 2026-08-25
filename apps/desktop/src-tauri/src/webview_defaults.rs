// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! This module removes browser-only behavior from application webviews.

use tauri::{Runtime, plugin::TauriPlugin};
use tauri_plugin_prevent_default::{Builder, Flags};

#[cfg(target_os = "macos")]
use tauri_plugin_prevent_default::{
    KeyboardShortcut,
    ModifierKey::{AltKey, MetaKey},
};

fn flags_for(debug: bool) -> Flags {
    let app_flags = Flags::CONTEXT_MENU
        | Flags::FIND
        | Flags::CARET_BROWSING
        | Flags::DOWNLOADS
        | Flags::SOURCE
        | Flags::OPEN
        | Flags::PRINT;

    if debug {
        app_flags
    } else {
        app_flags | Flags::DEV_TOOLS | Flags::RELOAD
    }
}

pub(super) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let debug = cfg!(debug_assertions);
    let builder = Builder::new().with_flags(flags_for(debug));

    #[cfg(target_os = "macos")]
    let builder = add_macos_shortcuts(builder, debug);

    builder.build()
}

#[cfg(target_os = "macos")]
fn add_macos_shortcuts(mut builder: Builder, debug: bool) -> Builder {
    for shortcut in macos_shortcuts(debug) {
        builder = builder.shortcut(shortcut);
    }
    builder
}

#[cfg(target_os = "macos")]
fn macos_shortcuts(debug: bool) -> Vec<KeyboardShortcut> {
    let mut shortcuts = vec![
        KeyboardShortcut::with_meta("f"),
        KeyboardShortcut::with_meta("g"),
        KeyboardShortcut::with_shift_meta("g"),
        KeyboardShortcut::with_shift_meta("j"),
        KeyboardShortcut::with_modifiers("l", &[AltKey, MetaKey]),
        KeyboardShortcut::with_modifiers("u", &[AltKey, MetaKey]),
        KeyboardShortcut::with_meta("o"),
        KeyboardShortcut::with_meta("p"),
        KeyboardShortcut::with_shift_meta("p"),
    ];

    if !debug {
        shortcuts.extend([
            KeyboardShortcut::with_modifiers("c", &[AltKey, MetaKey]),
            KeyboardShortcut::with_modifiers("i", &[AltKey, MetaKey]),
            KeyboardShortcut::with_modifiers("j", &[AltKey, MetaKey]),
            KeyboardShortcut::with_meta("r"),
            KeyboardShortcut::with_shift_meta("r"),
        ]);
    }

    shortcuts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    use tauri_plugin_prevent_default::ModifierKey::{AltKey, MetaKey, ShiftKey};

    #[test]
    fn release_blocks_browser_defaults_but_preserves_focus_movement() {
        let flags = flags_for(false);
        let expected = Flags::CONTEXT_MENU
            | Flags::FIND
            | Flags::CARET_BROWSING
            | Flags::DEV_TOOLS
            | Flags::DOWNLOADS
            | Flags::RELOAD
            | Flags::SOURCE
            | Flags::OPEN
            | Flags::PRINT;

        assert_eq!(flags, expected);
        assert!(flags.contains(Flags::CONTEXT_MENU));
        assert!(flags.contains(Flags::FIND));
        assert!(flags.contains(Flags::CARET_BROWSING));
        assert!(flags.contains(Flags::DEV_TOOLS));
        assert!(flags.contains(Flags::DOWNLOADS));
        assert!(flags.contains(Flags::RELOAD));
        assert!(flags.contains(Flags::SOURCE));
        assert!(flags.contains(Flags::OPEN));
        assert!(flags.contains(Flags::PRINT));
        assert!(!flags.contains(Flags::FOCUS_MOVE));
    }

    #[test]
    fn debug_keeps_reload_and_developer_tools_available() {
        let flags = flags_for(true);
        let expected = Flags::CONTEXT_MENU
            | Flags::FIND
            | Flags::CARET_BROWSING
            | Flags::DOWNLOADS
            | Flags::SOURCE
            | Flags::OPEN
            | Flags::PRINT;

        assert_eq!(flags, expected);
        assert!(flags.contains(Flags::CONTEXT_MENU));
        assert!(!flags.contains(Flags::DEV_TOOLS));
        assert!(!flags.contains(Flags::RELOAD));
        assert!(!flags.contains(Flags::FOCUS_MOVE));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn release_blocks_macos_browser_shortcuts() {
        let shortcuts = macos_shortcuts(false);

        assert!(has_shortcut(&shortcuts, "f", &[MetaKey]));
        assert!(has_shortcut(&shortcuts, "j", &[ShiftKey, MetaKey]));
        assert!(has_shortcut(&shortcuts, "l", &[AltKey, MetaKey]));
        assert!(has_shortcut(&shortcuts, "u", &[AltKey, MetaKey]));
        assert!(has_shortcut(&shortcuts, "o", &[MetaKey]));
        assert!(has_shortcut(&shortcuts, "p", &[MetaKey]));
        assert!(has_shortcut(&shortcuts, "r", &[MetaKey]));
        assert!(has_shortcut(&shortcuts, "i", &[AltKey, MetaKey]));
        assert!(!shortcuts.iter().any(|shortcut| shortcut.key() == "Tab"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn debug_keeps_macos_reload_and_developer_tools_available() {
        let shortcuts = macos_shortcuts(true);

        assert!(has_shortcut(&shortcuts, "f", &[MetaKey]));
        assert!(has_shortcut(&shortcuts, "p", &[MetaKey]));
        assert!(!has_shortcut(&shortcuts, "r", &[MetaKey]));
        assert!(!has_shortcut(&shortcuts, "i", &[AltKey, MetaKey]));
        assert!(!shortcuts.iter().any(|shortcut| shortcut.key() == "Tab"));
    }

    #[cfg(target_os = "macos")]
    fn has_shortcut(
        shortcuts: &[KeyboardShortcut],
        key: &str,
        modifiers: &[tauri_plugin_prevent_default::ModifierKey],
    ) -> bool {
        shortcuts
            .iter()
            .any(|shortcut| shortcut.key() == key && shortcut.modifiers() == modifiers)
    }
}
