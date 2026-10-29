//! Safe edits to existing coding-agent settings.

mod config;
pub mod current_context;
mod editor;
pub(crate) mod filesystem;
mod inventory;
#[path = "../opencode_config_authority.rs"]
pub(crate) mod opencode_config_authority;
pub(crate) mod opencode_current;
mod vendors;

pub use config::{
    ApplyConflict, ApplyError, ApplyReadbackError, ConfigChange, ConfigContext, ConfigOperation,
    ConfigOperationValue, ConfigScope, ConfigSetting, ConfigUnavailableReason, EffectiveConfig,
    EffectiveModel, PhysicalSelector, PreparedChange, PreparedOperation,
};
pub use editor::AgentConfigEditor;
pub use inventory::{
    AdvisoryResource, EnabledState, IndexedResourceEvidence, InventoryIssue, InventoryIssueReason,
    ResourceInventory, ResourceKind, ResourceProvenance, ResourceScope,
    advisory_resource_inventory, skill_definitions,
};

#[cfg(test)]
mod opencode_v2_tests;
#[cfg(test)]
mod tests;
