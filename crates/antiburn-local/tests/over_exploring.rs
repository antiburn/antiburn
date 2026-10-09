// Compile the check-specific unit tests in this focused test target.
pub use antiburn_local::{analysis, checks};

#[path = "../src/checks/over_exploring/mod.rs"]
pub mod over_exploring;
