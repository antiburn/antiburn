//! Parsing and typed access for authored Burn Check configuration.

pub mod yaml;

pub use yaml::{ConfigEntry, ConfigValue, ConfigValueKind, YamlDocument, parse_yaml};
