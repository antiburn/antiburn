//! Stable diagnostic codes and source locations for configuration failures.

use std::path::PathBuf;

use serde::Serialize;

/// A byte and line range in one YAML source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    /// The inclusive byte offset at the start of the range.
    pub start: usize,
    /// The exclusive byte offset at the end of the range.
    pub end: usize,
    /// The one-based source line at the start of the range.
    pub line: usize,
    /// The one-based source column at the start of the range.
    pub column: usize,
    /// The one-based source line at the end of the range.
    pub end_line: usize,
    /// The one-based source column at the end of the range.
    pub end_column: usize,
}

impl SourceSpan {
    /// Creates an empty location at one byte offset.
    #[must_use]
    pub const fn point(offset: usize, line: usize, column: usize) -> Self {
        Self {
            start: offset,
            end: offset,
            line,
            column,
            end_line: line,
            end_column: column,
        }
    }
}

/// A stable machine-readable diagnostic code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum DiagnosticCode {
    /// YAML syntax is malformed.
    YamlSyntax,
    /// A YAML directive is not part of the accepted check format.
    YamlDirective,
    /// The source has more than one YAML document.
    YamlDocumentCount,
    /// The document root is not a mapping.
    YamlRootType,
    /// A YAML collection is nested too deeply for safe compilation.
    YamlNestingLimit,
    /// A YAML anchor is not allowed.
    YamlAnchor,
    /// A YAML alias is not allowed.
    YamlAlias,
    /// A tag is outside the accepted JSON-compatible tag set.
    YamlUnsupportedTag,
    /// A mapping key does not resolve to a string.
    YamlNonStringKey,
    /// A mapping contains the same key more than once.
    YamlDuplicateKey,
    /// A mapping uses a merge key.
    YamlMergeKey,
    /// A scalar resolves to a non-finite number.
    YamlNonFiniteNumber,
    /// A scalar does not match its explicit YAML type.
    YamlInvalidScalar,
    /// A required configuration field is missing.
    ConfigMissingField,
    /// A configuration object contains an unknown field.
    ConfigUnknownField,
    /// A configuration field has the wrong JSON type.
    ConfigInvalidType,
    /// A configuration field has an invalid value or relationship.
    ConfigInvalidValue,
    /// The definition uses a feature not supported by this release.
    CompileUnsupportedFeature,
    /// An expression does not match the supported expression grammar.
    CompileInvalidExpression,
    /// An expression names a field unavailable in its evaluation context.
    CompileUnknownReference,
    /// A window does not satisfy the supported size and minimum rules.
    CompileInvalidWindow,
    /// A cooldown is not a valid bounded duration.
    CompileInvalidCooldown,
    /// A decision does not match the supported Choice contract.
    CompileInvalidDecision,
    /// An action does not match the supported status-action contract.
    CompileInvalidAction,
    /// A provider result does not satisfy the declared decision contract.
    RuntimeInvalidDecision,
    /// A frozen evaluation context violates the runtime contract.
    RuntimeInvalidContext,
    /// Expression evaluation exceeded a configured resource limit.
    EvaluationLimitExceeded,
    /// A filename does not have a valid lowercase check ID.
    DiscoveryInvalidId,
    /// Multiple files define the same ID in one installation scope.
    DiscoveryDuplicateId,
    /// A definition file or its directory cannot be read completely.
    DiscoveryReadFailed,
    /// A selected project override is paused.
    DiscoveryPaused,
    /// A project definition takes precedence over a global definition.
    DiscoveryShadowed,
}

impl DiagnosticCode {
    /// Returns the stable kebab-case code used by logs and interfaces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YamlSyntax => "yaml-syntax",
            Self::YamlDirective => "yaml-directive",
            Self::YamlDocumentCount => "yaml-document-count",
            Self::YamlRootType => "yaml-root-type",
            Self::YamlNestingLimit => "yaml-nesting-limit",
            Self::YamlAnchor => "yaml-anchor",
            Self::YamlAlias => "yaml-alias",
            Self::YamlUnsupportedTag => "yaml-unsupported-tag",
            Self::YamlNonStringKey => "yaml-non-string-key",
            Self::YamlDuplicateKey => "yaml-duplicate-key",
            Self::YamlMergeKey => "yaml-merge-key",
            Self::YamlNonFiniteNumber => "yaml-non-finite-number",
            Self::YamlInvalidScalar => "yaml-invalid-scalar",
            Self::ConfigMissingField => "config-missing-field",
            Self::ConfigUnknownField => "config-unknown-field",
            Self::ConfigInvalidType => "config-invalid-type",
            Self::ConfigInvalidValue => "config-invalid-value",
            Self::CompileUnsupportedFeature => "compile-unsupported-feature",
            Self::CompileInvalidExpression => "compile-invalid-expression",
            Self::CompileUnknownReference => "compile-unknown-reference",
            Self::CompileInvalidWindow => "compile-invalid-window",
            Self::CompileInvalidCooldown => "compile-invalid-cooldown",
            Self::CompileInvalidDecision => "compile-invalid-decision",
            Self::CompileInvalidAction => "compile-invalid-action",
            Self::RuntimeInvalidDecision => "runtime-invalid-decision",
            Self::RuntimeInvalidContext => "runtime-invalid-context",
            Self::EvaluationLimitExceeded => "evaluation-limit-exceeded",
            Self::DiscoveryInvalidId => "discovery-invalid-id",
            Self::DiscoveryDuplicateId => "discovery-duplicate-id",
            Self::DiscoveryReadFailed => "discovery-read-failed",
            Self::DiscoveryPaused => "discovery-paused",
            Self::DiscoveryShadowed => "discovery-shadowed",
        }
    }
}

/// One actionable parse, compile, or discovery diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// The stable machine-readable code.
    pub code: DiagnosticCode,
    /// The short human-readable explanation.
    pub message: String,
    /// The YAML source range, when the issue belongs to document content.
    pub span: Option<SourceSpan>,
    /// The logical configuration path, such as `on.window.min`.
    pub config_path: Option<String>,
    /// The file that supplied the definition, when discovery adds this context.
    pub source_path: Option<PathBuf>,
}

impl Diagnostic {
    /// Creates a diagnostic at a YAML source range.
    #[must_use]
    pub fn at(code: DiagnosticCode, message: impl Into<String>, span: SourceSpan) -> Self {
        Self {
            code,
            message: message.into(),
            span: Some(span),
            config_path: None,
            source_path: None,
        }
    }

    /// Creates a diagnostic without a YAML source range.
    #[must_use]
    pub fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span: None,
            config_path: None,
            source_path: None,
        }
    }

    /// Adds the authored configuration path to this diagnostic.
    #[must_use]
    pub fn with_config_path(mut self, path: impl Into<String>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Adds the definition file path to this diagnostic.
    #[must_use]
    pub fn with_source_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.source_path = Some(path.into());
        self
    }
}
