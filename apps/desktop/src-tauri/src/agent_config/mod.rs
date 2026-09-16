//! Safe edits to existing coding-agent settings.

mod config;
mod editor;
mod filesystem;
mod vendors;

pub use config::{
    ApplyConflict, ApplyError, ApplyReadbackError, ConfigChange, ConfigContext, ConfigOperation,
    ConfigOperationValue, ConfigScope, ConfigSetting, ConfigUnavailableReason, EffectiveConfig,
    EffectiveModel, PhysicalSelector, PreparedChange, PreparedOperation,
};
pub use editor::AgentConfigEditor;

#[cfg(test)]
mod tests;
