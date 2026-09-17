//! Safe edits to existing coding-agent settings.

mod config;
mod editor;
mod filesystem;
mod inventory;
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
    advisory_resource_inventory,
};

#[cfg(test)]
mod tests;
