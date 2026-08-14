// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Update checks against the public GitHub Releases feed.
//!
//! The updater plugin is the only network-capable surface in the whole
//! application, and this module owns every decision about when it is allowed to
//! speak:
//!
//! - **Whether it can speak at all.** [`supported`] answers from *real*
//!   registration state — a flag set only after the plugin actually built and
//!   installed — plus a non-empty signing public key in the bundled
//!   configuration. A compile-time `cfg!` would say "yes" in a release build
//!   whose key was never configured, and every piece of copy downstream would
//!   inherit that lie.
//! - **When it speaks on its own.** [`spawn_scheduler`] checks once shortly
//!   after launch and then every [`CHECK_INTERVAL`], and only while the reader's
//!   "check automatically" preference is on. It is the thing that makes that
//!   preference real; without it the switch would be decoration.
//! - **What it says afterwards.** Every check — automatic or not — ends in an
//!   [`EVENT_UPDATE`] event carrying an [`UpdateStatus`], which is what the
//!   Updates pane renders. Nothing is inferred from silence. An *automatic*
//!   check that finds a version also asks [`crate::notifications`] to say so
//!   once, since nobody is watching the pane when the schedule runs.
//!
//! Nothing here contacts a server in a development build: the plugin is never
//! registered there, so [`supported`] is false and the scheduler does nothing.
//!
//! # The signing key is a release requirement, not a nicety
//!
//! `plugins.updater.pubkey` in `tauri.conf.json` carries the updater key's
//! public half (minted 2026-08-14; custody in
//! `docs/runbooks/updater-key-recovery.md`), and
//! `bundle.createUpdaterArtifacts` is **true** — so a release build produces
//! signed updater bundles, and this build claims update support only while a
//! key is present. The two halves are one decision: an updater with artifacts
//! but no key would download something it cannot verify, and an updater with a
//! key but no artifacts would have nothing to check.
//!
//! A real release therefore **requires** the key pair to exist: the private
//! half in the release environment as `TAURI_SIGNING_PRIVATE_KEY`, the public
//! half committed here. `.github/workflows/release-app.yml` fails the release
//! outright while the field is empty rather than shipping a build whose update
//! path is decoration. The key was minted 2026-08-14 — custody, rotation, and
//! what a reader has to do if it is ever lost are in
//! `docs/runbooks/updater-key-recovery.md`.
//!
//! (The warning lives here rather than beside the field because `tauri.conf.json`
//! is parsed as strict JSON — `tauri-build`'s default features exclude
//! `config-json5` — so a comment in that file would fail the build.)

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::store::Store;

/// Event the shell emits after every update check.
pub const EVENT_UPDATE: &str = "update:status";

/// How long after launch the first automatic check runs.
///
/// Launch is the busiest moment in the app's life — the first scan, the store
/// migration, the webview boot — and an update check is the least urgent thing
/// competing for it.
pub const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// How often the automatic check repeats.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Whether the updater plugin actually registered.
///
/// Registered as Tauri managed state and set exactly once, from
/// `install_updater`. Its default — false — is what a development build, a
/// build with no signing key, and a build whose plugin failed to install all
/// report, which is the truth in every one of those cases.
#[derive(Default)]
pub struct UpdaterState {
    registered: AtomicBool,
}

impl UpdaterState {
    /// Called after the plugin installed successfully. Not public guesswork:
    /// the only caller is the registration site itself.
    ///
    /// That site is compiled into release builds only, so in a development
    /// build nothing calls this outside the tests — which is precisely the
    /// state it exists to describe.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub fn note_registered(&self) {
        self.registered.store(true, Ordering::SeqCst);
    }

    pub fn registered(&self) -> bool {
        self.registered.load(Ordering::SeqCst)
    }
}

/// Whether this build can check for updates *at all*.
///
/// Both halves matter. Without registration there is no plugin to ask; without
/// a signing key the plugin cannot verify what it downloads, so a check that
/// appeared to work would be a check whose answer could not be trusted.
pub fn supported(app: &AppHandle) -> bool {
    app.try_state::<UpdaterState>()
        .is_some_and(|state| state.registered())
        && signing_key_configured(app)
}

/// Whether the bundled configuration carries an updater public key.
pub fn signing_key_configured(app: &AppHandle) -> bool {
    let plugins = serde_json::to_value(&app.config().plugins).unwrap_or(serde_json::Value::Null);
    has_signing_key(&plugins)
}

/// The pure half of [`signing_key_configured`], over the serialized plugin
/// configuration.
fn has_signing_key(plugins: &serde_json::Value) -> bool {
    plugins
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

/// The outcome of one update check, as the Updates pane renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// `current`, `available`, `failed`, or `unsupported`.
    pub kind: &'static str,
    /// The version waiting to be installed, when there is one.
    pub version: Option<String>,
    /// Why a check failed, in the plugin's own words.
    pub message: Option<String>,
    /// ISO-8601 stamp of the moment the check finished.
    pub checked_at: String,
    /// True when the check ran on the schedule rather than because a reader
    /// pressed a button — so the pane can say "checked automatically" without
    /// inventing the distinction.
    pub automatic: bool,
}

impl UpdateStatus {
    fn new(kind: &'static str, automatic: bool) -> UpdateStatus {
        UpdateStatus {
            kind,
            version: None,
            message: None,
            checked_at: crate::store::now_rfc3339(),
            automatic,
        }
    }

    fn current(automatic: bool) -> UpdateStatus {
        UpdateStatus::new("current", automatic)
    }

    fn available(version: String, automatic: bool) -> UpdateStatus {
        UpdateStatus {
            version: Some(version),
            ..UpdateStatus::new("available", automatic)
        }
    }

    fn failed(message: String, automatic: bool) -> UpdateStatus {
        UpdateStatus {
            message: Some(message),
            ..UpdateStatus::new("failed", automatic)
        }
    }
}

/// Start the automatic check schedule. The returned handle is aborted on exit.
///
/// The loop runs in every build; what it *does* is decided every pass by
/// [`supported`] and the reader's preference, so there is no compile-time
/// branch to get wrong and the whole path is covered by the normal checks.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            if should_check(&app) {
                let status = check(&app, true).await;
                // The Updates pane learns from the event; someone who is not
                // looking at antiburn learns from a notification, at most once
                // per version. A *manual* check is deliberately not notified:
                // the reader is already looking at the answer.
                crate::notifications::note_update_status(&app, &status);
                let _ = app.emit(EVENT_UPDATE, status);
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    })
}

/// Whether an automatic check should run right now.
fn should_check(app: &AppHandle) -> bool {
    supported(app) && auto_update_enabled(app)
}

/// The reader's "check automatically" preference, defaulting to *not* checking
/// when the store cannot be read: a failed read is not consent.
fn auto_update_enabled(app: &AppHandle) -> bool {
    app.try_state::<Store>()
        .and_then(|store| store.settings().ok())
        .is_some_and(|settings| settings.auto_update)
}

/// Ask the release feed whether there is a newer version.
///
/// Guarded by [`supported`] rather than by a `cfg`, because the plugin's own
/// extension trait reads managed state that only exists once the plugin has
/// registered.
pub async fn check(app: &AppHandle, automatic: bool) -> UpdateStatus {
    if !supported(app) {
        return UpdateStatus::new("unsupported", automatic);
    }
    check_with_plugin(app, automatic).await
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn check_with_plugin(app: &AppHandle, automatic: bool) -> UpdateStatus {
    use tauri_plugin_updater::UpdaterExt as _;

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => return UpdateStatus::failed(error.to_string(), automatic),
    };
    match updater.check().await {
        Ok(Some(update)) => UpdateStatus::available(update.version.clone(), automatic),
        Ok(None) => UpdateStatus::current(automatic),
        Err(error) => UpdateStatus::failed(error.to_string(), automatic),
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
async fn check_with_plugin(_app: &AppHandle, automatic: bool) -> UpdateStatus {
    UpdateStatus::new("unsupported", automatic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_public_key_is_not_a_configured_one() {
        let plugins = serde_json::json!({ "updater": { "pubkey": "" } });
        assert!(!has_signing_key(&plugins));

        let blank = serde_json::json!({ "updater": { "pubkey": "   " } });
        assert!(!has_signing_key(&blank));
    }

    #[test]
    fn a_real_public_key_is_configured() {
        let plugins = serde_json::json!({
            "updater": { "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk=" }
        });
        assert!(has_signing_key(&plugins));
    }

    #[test]
    fn a_configuration_with_no_updater_block_carries_no_key() {
        assert!(!has_signing_key(&serde_json::json!({})));
        assert!(!has_signing_key(&serde_json::json!({ "updater": {} })));
        assert!(!has_signing_key(&serde_json::Value::Null));
        // A non-string key is malformed, not a key.
        assert!(!has_signing_key(
            &serde_json::json!({ "updater": { "pubkey": 42 } })
        ));
    }

    #[test]
    fn a_fresh_updater_state_reports_the_truth_about_a_development_build() {
        let state = UpdaterState::default();
        assert!(
            !state.registered(),
            "nothing has registered, so nothing may claim update support"
        );
        state.note_registered();
        assert!(state.registered());
    }

    #[test]
    fn every_outcome_serializes_as_the_shape_the_pane_renders() {
        let available = UpdateStatus::available("0.2.0".into(), true);
        let json = serde_json::to_value(&available).unwrap();
        assert_eq!(json["kind"], "available");
        assert_eq!(json["version"], "0.2.0");
        assert_eq!(json["automatic"], true);
        assert!(json["checkedAt"].as_str().is_some_and(|at| at.len() >= 20));

        let failed =
            serde_json::to_value(UpdateStatus::failed("no network".into(), false)).unwrap();
        assert_eq!(failed["kind"], "failed");
        assert_eq!(failed["message"], "no network");
        assert_eq!(failed["automatic"], false);

        let current = serde_json::to_value(UpdateStatus::current(false)).unwrap();
        assert_eq!(current["kind"], "current");
        assert!(current["version"].is_null());
    }

    #[test]
    fn the_schedule_is_the_one_the_updates_pane_describes() {
        // The pane tells the reader an automatic check happens a moment after
        // launch and then every few hours; these are those numbers.
        assert_eq!(STARTUP_DELAY, Duration::from_secs(30));
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(21_600));
    }
}
