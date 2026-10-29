//! Compatibility exports for native tool-result publication and delivery.

pub(crate) mod admission;
pub(crate) mod evidence;
pub(crate) mod observers;
pub(crate) mod opencode_v2_status;
pub(crate) mod publications;
pub(crate) mod request_origin;
pub(crate) mod source_delivery;
pub(crate) mod trace_effects;

#[cfg(test)]
#[path = "native_tool_results/main_64_preservation_tests.rs"]
mod main_64_preservation_tests;

pub use publications::{
    ToolResultBatch, ToolResultPublication, ToolResultPublicationOutcome, ToolResultPublishOptions,
};
pub(crate) use publications::{clear_local_data_in, delete_session_data_in, reconcile_batch_in};
