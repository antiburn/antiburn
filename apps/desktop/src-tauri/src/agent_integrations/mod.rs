//! Per-user installation of the existing JavaScript session-check adapters.

pub mod artifact;
pub mod desktop;
pub mod discovery;
pub mod document;
pub mod lifecycle;
pub mod manager;
#[cfg(target_os = "macos")]
mod metadata;
pub mod process;
pub mod transaction;

#[cfg(all(test, unix))]
mod tests;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Claude,
    Codex,
    Cursor,
    OpenCode,
    Pi,
}

impl Vendor {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::OpenCode,
        Self::Pi,
    ];

    pub const fn needs_node(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Cursor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationPhase {
    NotDetected,
    Unavailable,
    PendingNativeGate,
    Registered,
    AwaitingTrust,
    ReloadRequired,
    Active,
    UserChanged,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    pub vendor: Vendor,
    pub phase: IntegrationPhase,
    pub reason: String,
    pub host_version: Option<String>,
    pub profile: Option<PathBuf>,
    pub node_version: Option<String>,
    pub owned: bool,
}
