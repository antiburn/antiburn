//! Reusable native infrastructure for a window placed beside another window.
//!
//! The manager owns one renderer, one target, and one cancellable task. The
//! host application owns the target type, IPC authorization, and data policy.

mod geometry;
mod lifecycle;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod manager;
mod model;
mod platform;
#[cfg(target_os = "windows")]
mod windows;

pub use manager::AnchoredWindowManager;
pub use model::{
    AnchorRegion, AnchoredWindowConfig, AnchoredWindowLifecycleEvent, AnchoredWindowRenderRequest,
    AnchoredWindowRequest, AnchoredWindowState, HeightPolicy, InteractionPolicy, PlacementPolicy,
    PointerExitPolicy, RevealPolicy, WindowMaterial,
};

/// The event that carries each target generation to the resident renderer.
pub const REQUEST_EVENT: &str = "anchored-window-request";

/// The event that reports companion lifecycle changes to the anchor window.
pub const STATE_EVENT: &str = "anchored-window-state";
