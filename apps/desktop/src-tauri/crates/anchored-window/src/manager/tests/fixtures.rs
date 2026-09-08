use std::time::Duration;

use crate::model::AnchoredWindowConfig;

use super::super::AnchoredWindowManager;

pub(super) fn config() -> AnchoredWindowConfig {
    AnchoredWindowConfig {
        label: "companion".to_string(),
        anchor_label: "anchor".to_string(),
        route: "index.html#/companion".to_string(),
        title: "companion".to_string(),
        width: 320.0,
        corner_radius: 8.0,
        initial_height: 120.0,
        min_height: 60.0,
        max_height: 320.0,
        gap: 8.0,
        screen_margin: 8.0,
        conceal_fallback: Duration::from_millis(80),
        pointer_exit: None,
    }
}

pub(super) fn manager() -> AnchoredWindowManager<&'static str> {
    AnchoredWindowManager::new(config())
}
