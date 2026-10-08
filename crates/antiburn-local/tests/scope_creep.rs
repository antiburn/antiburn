// Compile the check in isolation against the engine's public dependencies.
pub use antiburn_local::{analysis, checks};

#[path = "../src/checks/scope_creep/mod.rs"]
pub mod scope_creep;
