//! Strict Burn Check parsing, compilation, and pure evaluation.
//!
//! This crate owns no filesystem watcher, provider transport, persistence, or
//! desktop integration. [`compile`] turns one YAML document into an immutable
//! supported definition or returns source-located diagnostics.

pub mod actions;
pub mod bounded_json;
pub mod check_store;
pub mod compiler;
pub mod config;
pub mod context;
pub mod decision;
pub mod definition_lifecycle;
pub mod diagnostics;
pub mod discovery;
pub mod domain;
pub mod engine;
pub mod expression;
pub mod expression_eval;
pub mod filters;
pub mod fit;
pub mod preparation;
pub mod prepared_assessment;
pub mod replay;
pub mod results;
pub mod selection;
pub mod source;

pub use compiler::{CompiledCheck, compile, compile_document};
pub use config::{ConfigEntry, ConfigValue, ConfigValueKind, YamlDocument, parse_yaml};
pub use diagnostics::{Diagnostic, DiagnosticCode, SourceSpan};
pub use discovery::{
    CheckFile, DefinitionScope, DefinitionState, DiscoveryConfig, DiscoveryResult,
    EffectiveDefinition, discover_definitions,
};
