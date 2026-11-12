//! Open isolated Cloud previews from the debug tray menu.

use tauri::menu::{IsMenuItem, MenuItem, Submenu};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

const LABEL: &str = "cloud-dev";
const SCENARIOS: &[(&str, &str)] = &[
    ("walkthrough", "Full Walkthrough"),
    ("marketing", "Learn More"),
    ("auth", "GitHub Sign-in"),
    ("waiting", "Waiting for GitHub"),
    ("cancelled", "Sign-in Cancelled"),
    ("expired", "Sign-in Expired"),
    ("browser_failed", "Browser Launch Failed"),
    ("offline", "Offline"),
    ("rejected", "Sign-in Rejected"),
    ("invalid_response", "Invalid Auth Response"),
    ("credential_failed", "Credential Storage Failed"),
    ("identity", "Signed In"),
    ("folders", "Choose Folders"),
    ("empty", "No Folders"),
    ("unavailable", "Sync Unavailable"),
    ("complete-ready", "Completion — Agent Ready"),
    ("complete-pending", "Completion — Agent Pending"),
    ("complete-unavailable", "Completion — Agent Unavailable"),
    ("complete-failed", "Completion — Agent Failed"),
];

pub(crate) fn menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let entries = SCENARIOS
        .iter()
        .map(|(key, label)| {
            MenuItem::with_id(app, format!("cloud-dev:{key}"), label, true, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let items: Vec<&dyn IsMenuItem<Wry>> = entries
        .iter()
        .map(|item| item as &dyn IsMenuItem<Wry>)
        .collect();
    Submenu::with_id_and_items(app, LABEL, "Cloud Dev", true, &items)
}

pub(crate) fn handle(app: &AppHandle, id: &str) -> bool {
    let Some(scenario) = id.strip_prefix("cloud-dev:") else {
        return false;
    };
    if !SCENARIOS.iter().any(|(key, _)| *key == scenario) {
        return false;
    }
    if let Err(error) = open(app, scenario) {
        tracing::warn!(event = "cloud_dev_open_failed", error = %error);
    }
    true
}

fn open(app: &AppHandle, scenario: &str) -> tauri::Result<()> {
    let value = serde_json::to_string(scenario)?;
    if let Some(window) = app.get_webview_window(LABEL) {
        window.eval(format!(
            "window.__ANTIBURN_CLOUD_DEV__={value};window.dispatchEvent(new CustomEvent('cloud-dev:scenario', {{detail:{value}}}))"
        ))?;
        if window.is_visible()? || window.is_minimized()? {
            window.unminimize()?;
            window.set_focus()?;
        }
        return Ok(());
    }
    let scale = crate::interface_scale::current(app);
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("onboarding.html".into()))
        .title("Cloud Dev — Simulated")
        .initialization_script(crate::interface_scale::append_initialization_script(
            format!(
                "window.__ANTIBURN_CLOUD_DEV__={value};window.__ANTIBURN_WINDOW_GENERATION__=1;"
            ),
            scale,
        ))
        .inner_size(860.0 * scale.factor(), 620.0 * scale.factor())
        .visible(false)
        .resizable(false)
        .maximizable(false)
        .zoom_hotkeys_enabled(false);
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay);
    let window = builder.build()?;
    crate::interface_scale::apply_window(&window, scale)?;
    crate::window_placement::center_on_active_monitor(
        &window,
        860.0 * scale.factor(),
        620.0 * scale.factor(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_capability_has_no_cloud_or_onboarding_mutations() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/cloud-dev.json")).unwrap();
        assert_eq!(capability["windows"], serde_json::json!([LABEL]));
        for permission in capability["permissions"].as_array().unwrap() {
            let permission = permission.as_str().unwrap();
            assert!(
                (!permission.starts_with("allow-cloud-")
                    || permission == "allow-cloud-preview-window")
                    && !permission.contains("onboarding")
            );
            assert!(!permission.starts_with("core:window:"));
        }
    }
}
