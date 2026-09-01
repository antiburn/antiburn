use std::time::Duration;

use crate::model::{
    AnchoredWindowConfig, HeightPolicy, InteractionPolicy, PlacementPolicy, RevealPolicy,
    WindowMaterial,
};

use super::super::AnchoredWindowManager;

pub(super) fn config() -> AnchoredWindowConfig {
    AnchoredWindowConfig {
        label: "companion".to_string(),
        anchor_label: "anchor".to_string(),
        route: "index.html#/companion".to_string(),
        title: "companion".to_string(),
        width: 320.0,
        material: WindowMaterial::Transparent,
        interaction: InteractionPolicy::Passive,
        reveal: RevealPolicy::AfterPresentation,
        height: HeightPolicy::Content {
            initial: 120.0,
            min: 60.0,
            max: 320.0,
        },
        placement: PlacementPolicy::LeftPreferred {
            gap: 8.0,
            screen_margin: 8.0,
        },
        conceal_fallback: Duration::from_millis(80),
    }
}

pub(super) fn manager() -> AnchoredWindowManager<&'static str> {
    AnchoredWindowManager::new(config())
}
