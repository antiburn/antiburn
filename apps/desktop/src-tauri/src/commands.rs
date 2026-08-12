// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The IPC surface exposed to the webview.
//!
//! Commands stay thin: they translate a request into an engine or window-system
//! call and map the result into something serializable. Anything that needs
//! real logic belongs in the engine.

use crate::settings;

/// Version stamp of the engine's bundled pricing catalog (`YYYY-MM-DD`).
///
/// Small on purpose, and load-bearing: it is the shell's end-to-end proof that
/// the webview, the IPC bridge, and the linked engine all work.
#[tauri::command]
pub fn engine_catalog_version() -> &'static str {
    antiburn_local::pricing::PRICING_CATALOG_VERSION
}

/// Opens, or refocuses, the standalone settings window.
#[tauri::command]
pub fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    settings::open(&app).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_version_comes_from_the_engine_and_is_a_review_date() {
        let version = engine_catalog_version();
        assert_eq!(version, antiburn_local::pricing::PRICING_CATALOG_VERSION);
        assert_eq!(version.len(), 10, "expected a YYYY-MM-DD review date");
        assert!(
            version
                .split('-')
                .all(|part| part.chars().all(|c| c.is_ascii_digit()))
        );
    }
}
