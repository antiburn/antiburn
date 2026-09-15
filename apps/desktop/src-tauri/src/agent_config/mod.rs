//! Safe edits to existing coding-agent settings.

mod config;
mod editor;
mod filesystem;
mod vendors;

pub use config::{
    ApplyConflict, ApplyError, ApplyReadbackError, ConfigChange, ConfigContext, ConfigOperation,
    ConfigScope, ConfigSetting, ConfigUnavailableReason, EffectiveConfig, EffectiveModel,
    PreparedChange,
};
pub use editor::AgentConfigEditor;

#[cfg(test)]
mod tests;
