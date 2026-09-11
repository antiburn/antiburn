use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Pointer tolerance after the pointer enters a companion window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerExitPolicy {
    /// The logical distance outside the native frame that still counts as inside.
    pub edge_tolerance: f64,
    /// The continuous time outside the tolerance before concealment starts.
    pub outside_delay: Duration,
}

/// A target element's logical bounds relative to the anchor window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorRegion {
    /// The logical distance from the anchor window's top edge.
    pub top: f64,
    /// The target element's logical height.
    pub height: f64,
}

impl AnchorRegion {
    pub(crate) fn sanitized(self) -> Self {
        Self {
            top: if self.top.is_finite() {
                self.top.max(0.0)
            } else {
                0.0
            },
            height: if self.height.is_finite() {
                self.height.max(0.0)
            } else {
                0.0
            },
        }
    }

    pub(crate) fn top_within(self, anchor_height: f64) -> f64 {
        self.sanitized().top.min(anchor_height.max(0.0))
    }
}

/// Native configuration for one anchored companion window.
#[derive(Clone, Debug)]
pub struct AnchoredWindowConfig {
    /// The companion window label.
    pub label: String,
    /// The anchor window label.
    pub anchor_label: String,
    /// The application route loaded by the companion renderer.
    pub route: String,
    /// The native companion window title.
    pub title: String,
    /// The companion's logical width.
    pub width: f64,
    /// The corner radius for the native popover material.
    pub corner_radius: f64,
    /// The logical height before the first renderer measurement.
    pub initial_height: f64,
    /// The minimum logical content height.
    pub min_height: f64,
    /// The maximum logical content height.
    pub max_height: f64,
    /// The logical gap between the anchor and companion.
    pub gap: f64,
    /// The minimum logical margin inside the screen work area.
    pub screen_margin: f64,
    /// The maximum wait for renderer-confirmed concealment.
    pub conceal_fallback: Duration,
    /// Optional pointer tolerance after the pointer enters the companion.
    pub pointer_exit: Option<PointerExitPolicy>,
}

/// One target request delivered to the companion renderer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredWindowRequest<T> {
    /// The current target generation.
    pub generation: u64,
    /// The current target, or `None` for concealment.
    pub target: Option<T>,
    /// Whether the renderer must confirm a visible retarget.
    pub retarget_commit_required: bool,
}

/// One renderer request with optional content supplied by the instigator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredWindowRenderRequest<T, P> {
    /// The current target generation.
    pub generation: u64,
    /// The current target, or `None` for concealment.
    pub target: Option<T>,
    /// Whether the renderer must confirm a visible retarget.
    pub retarget_commit_required: bool,
    /// Optional content supplied with the target request.
    pub initial_presentation: Option<P>,
}

/// The manager state exposed through a host-owned command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredWindowState<T> {
    /// The current target generation.
    pub generation: u64,
    /// The current target, or `None` during concealment.
    pub target: Option<T>,
    /// Whether the current renderer load is ready.
    pub renderer_ready: bool,
    /// Whether the lifecycle currently requests native companion visibility.
    pub visible: bool,
    /// Whether a visible retarget waits for renderer confirmation.
    pub awaiting_retarget_commit: bool,
    /// Whether the current target waits for initial presentation.
    pub awaiting_presentation: bool,
    /// Whether concealment waits for renderer confirmation.
    pub awaiting_concealment: bool,
}

/// One companion's typed lifecycle snapshot for its anchor window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredWindowLifecycleEvent<T> {
    /// The companion window label.
    pub companion_label: String,
    /// The companion's current lifecycle state.
    pub state: AnchoredWindowState<T>,
}

pub(crate) fn normalized_heights(initial: f64, min: f64, max: f64) -> (f64, f64, f64) {
    let min = finite_positive(min, 1.0);
    let max = finite_positive(max, min).max(min);
    let initial = finite_positive(initial, min).clamp(min, max);
    (initial, min, max)
}

fn finite_positive(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests;
