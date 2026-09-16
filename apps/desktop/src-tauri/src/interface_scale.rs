//! Persisted application interface scaling and native window reconciliation.

use std::sync::LazyLock;

use serde::Deserialize;
use tauri::{Emitter, Manager, WebviewWindow};

use crate::store::{AppSettings, Store};

pub const DEFAULT_PERCENT: u16 = 100;
pub const CHANGED_EVENT: &str = "antiburn:interface-scale-changed";

static PRESETS: LazyLock<Vec<u16>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../interface-scale.json"))
        .expect("interface-scale.json must contain interface scale presets")
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfaceScale(u16);

impl InterfaceScale {
    pub fn new(percent: u16) -> Option<Self> {
        presets().contains(&percent).then_some(Self(percent))
    }

    pub const fn percent(self) -> u16 {
        self.0
    }

    pub fn factor(self) -> f64 {
        f64::from(self.0) / 100.0
    }
}

impl Default for InterfaceScale {
    fn default() -> Self {
        Self(DEFAULT_PERCENT)
    }
}

pub fn presets() -> &'static [u16] {
    PRESETS.as_slice()
}

pub fn normalize_percent(percent: u16) -> u16 {
    InterfaceScale::new(percent).unwrap_or_default().percent()
}

pub fn from_settings(settings: &AppSettings) -> InterfaceScale {
    InterfaceScale::new(settings.interface_scale_percent).unwrap_or_default()
}

pub fn current(app: &tauri::AppHandle) -> InterfaceScale {
    app.try_state::<Store>()
        .map(|store| store.settings_snapshot())
        .as_ref()
        .map(from_settings)
        .unwrap_or_default()
}

pub fn initialization_script(scale: InterfaceScale) -> String {
    let percent = scale.percent();
    let factor = scale.factor();
    format!(
        r#"globalThis.__ANTIBURN_INTERFACE_SCALE_PERCENT__ = {percent};
const applyAntiburnInterfaceScale = () => document.documentElement?.style.setProperty('--interface-scale', '{factor}');
applyAntiburnInterfaceScale();
document.addEventListener('DOMContentLoaded', applyAntiburnInterfaceScale, {{ once: true }});"#
    )
}

pub fn append_initialization_script(script: String, scale: InterfaceScale) -> String {
    format!("{script}\n{}", initialization_script(scale))
}

/// Apply zoom before a hidden renderer can be revealed.
pub fn apply_window(window: &WebviewWindow, scale: InterfaceScale) -> tauri::Result<()> {
    let factor = scale.factor();
    window.set_zoom(factor)
}

fn update_script(scale: InterfaceScale) -> String {
    let percent = scale.percent();
    let factor = scale.factor();
    format!(
        r#"globalThis.__ANTIBURN_INTERFACE_SCALE_PERCENT__ = {percent};
document.documentElement?.style.setProperty('--interface-scale', '{factor}');
globalThis.dispatchEvent(new CustomEvent('{CHANGED_EVENT}', {{ detail: {{ percent: {percent} }} }}));"#
    )
}

/// Apply a saved scale to every renderer that already exists.
pub fn reconcile_existing(app: &tauri::AppHandle, scale: InterfaceScale) -> Result<(), String> {
    let mut errors = Vec::new();
    for (label, window) in app.webview_windows() {
        if let Err(error) = apply_window(&window, scale) {
            errors.push(format!("{label}: {error}"));
            continue;
        }
        if let Err(error) = window.eval(update_script(scale)) {
            errors.push(format!("{label}: {error}"));
        }
    }
    if let Err(error) = crate::onboarding::reconcile_interface_scale(app, scale) {
        errors.push(format!("{}: {error}", crate::onboarding::LABEL));
    }
    if let Err(error) = crate::settings::reconcile_interface_scale(app, scale) {
        errors.push(format!("{}: {error}", crate::settings::LABEL));
    }
    if let Some(manager) = app.try_state::<crate::popover_peek::PopoverPeekManager>()
        && let Err(error) = manager.set_interface_scale(app, scale.factor())
    {
        errors.push(format!("{}: {error}", crate::popover_peek::LABEL));
    }
    if let Err(error) = crate::popover::reconcile_interface_scale(app, scale.factor()) {
        errors.push(format!("{}: {error}", crate::popover::LABEL));
    }
    if let Some(manager) = app.try_state::<antiburn_nudge::NudgeManager>()
        && let Err(error) = manager.reconcile_interface_scale(scale.factor())
    {
        errors.push(format!("nudge: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn emit_settings_changed(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    app.emit(crate::commands::SETTINGS_CHANGED_EVENT, settings)
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InterfaceScaleChange {
    Set { percent: u16 },
    Increase,
    Decrease,
    Reset,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceScaleSource {
    Settings,
    Shortcut,
    Menu,
}

impl InterfaceScaleSource {
    pub const fn analytics_value(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Shortcut => "shortcut",
            Self::Menu => "menu",
        }
    }
}

pub fn resolve_change(current: u16, change: InterfaceScaleChange) -> Result<u16, String> {
    let index = presets()
        .iter()
        .position(|candidate| *candidate == current)
        .or_else(|| {
            presets()
                .iter()
                .position(|candidate| *candidate == DEFAULT_PERCENT)
        })
        .ok_or_else(|| "interface scale presets must include the default".to_owned())?;
    match change {
        InterfaceScaleChange::Set { percent } => InterfaceScale::new(percent)
            .map(InterfaceScale::percent)
            .ok_or_else(|| format!("unsupported interface scale preset: {percent}")),
        InterfaceScaleChange::Increase => Ok(presets()[(index + 1).min(presets().len() - 1)]),
        InterfaceScaleChange::Decrease => Ok(presets()[index.saturating_sub(1)]),
        InterfaceScaleChange::Reset => Ok(DEFAULT_PERCENT),
    }
}

pub fn analytics_preset(percent: u16) -> Option<&'static str> {
    match percent {
        90 => Some("90"),
        100 => Some("100"),
        110 => Some("110"),
        125 => Some("125"),
        150 => Some("150"),
        175 => Some("175"),
        200 => Some("200"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_presets_are_ordered_and_have_the_documented_default() {
        assert_eq!(presets(), &[90, 100, 110, 125, 150, 175, 200]);
        assert!(presets().contains(&DEFAULT_PERCENT));
    }

    #[test]
    fn relative_changes_stop_at_the_preset_edges() {
        assert_eq!(resolve_change(90, InterfaceScaleChange::Decrease), Ok(90));
        assert_eq!(resolve_change(200, InterfaceScaleChange::Increase), Ok(200));
        assert_eq!(resolve_change(125, InterfaceScaleChange::Reset), Ok(100));
        assert!(resolve_change(111, InterfaceScaleChange::Set { percent: 111 }).is_err());
    }
}
