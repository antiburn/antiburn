//! Source types shared by Desktop folder discovery and ongoing capture.

use antiburn_cloud_capture::CaptureSourceKind;
use serde::{Deserialize, Serialize};

#[path = "cloud_projects.rs"]
pub(crate) mod projects;
#[path = "cloud_sync.rs"]
pub(crate) mod sync;

/// Original provider source format, not an installed runtime or live permission.
/// Only absent legacy fields default to Claude; unknown values remain errors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    #[default]
    ClaudeJsonl,
    #[serde(rename = "opencode_sqlite_v2")]
    OpenCodeSqliteV2,
    CodexJsonl,
    PiJsonl,
    CursorIdeComposer,
    #[serde(rename = "cursor_cli_agent_jsonl")]
    CursorCliAgentJsonl,
}

impl SourceKind {
    /// Keep the existing serialized form of Claude roots.
    pub(crate) fn is_claude(&self) -> bool {
        *self == Self::ClaudeJsonl
    }

    /// Whether the source binding is a provider directory rather than a database file.
    pub(crate) fn is_directory(self) -> bool {
        self != Self::OpenCodeSqliteV2
    }

    /// Public API agent and format strings for this source.
    pub(crate) fn upload_names(self) -> (&'static str, &'static str) {
        match self {
            Self::ClaudeJsonl => ("claude-code", "ClaudeJsonl"),
            Self::OpenCodeSqliteV2 => ("opencode", "OpenCodeSqliteV2"),
            Self::CodexJsonl => ("codex", "CodexJsonl"),
            Self::PiJsonl => ("pi", "PiJsonl"),
            Self::CursorIdeComposer => ("cursor", "CursorIdeComposer"),
            Self::CursorCliAgentJsonl => ("cursor", "CursorCliAgentJsonl"),
        }
    }

    /// New file exporters require an explicit service capability.
    pub(crate) fn is_codex_or_pi(self) -> bool {
        matches!(self, Self::CodexJsonl | Self::PiJsonl)
    }

    /// These providers require a source root returned by project discovery.
    pub(crate) fn is_project_only(self) -> bool {
        self.is_codex_or_pi() || matches!(self, Self::CursorIdeComposer | Self::CursorCliAgentJsonl)
    }
}

impl From<CaptureSourceKind> for SourceKind {
    fn from(value: CaptureSourceKind) -> Self {
        match value {
            CaptureSourceKind::ClaudeJsonl => Self::ClaudeJsonl,
            CaptureSourceKind::OpenCodeSqliteV2 => Self::OpenCodeSqliteV2,
            CaptureSourceKind::CodexJsonl => Self::CodexJsonl,
            CaptureSourceKind::PiJsonl => Self::PiJsonl,
            CaptureSourceKind::CursorIdeComposer => Self::CursorIdeComposer,
            CaptureSourceKind::CursorCliAgentJsonl => Self::CursorCliAgentJsonl,
        }
    }
}

/// Exact approved source. Provider and path bind consent, not current access.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceSelection {
    pub source_kind: SourceKind,
    pub path: String,
}

/// Stable account/local-root identity, separate from replaceable access grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RootScope {
    pub path: String,
    pub source_scope_id: String,
    #[serde(default, skip_serializing_if = "SourceKind::is_claude")]
    pub source_kind: SourceKind,
}

#[cfg(test)]
#[test]
fn cloud_provider_wire_values_match_native_candidate_kinds() {
    for (kind, wire) in [
        (CaptureSourceKind::ClaudeJsonl, "\"claude_jsonl\""),
        (
            CaptureSourceKind::OpenCodeSqliteV2,
            "\"opencode_sqlite_v2\"",
        ),
        (CaptureSourceKind::CodexJsonl, "\"codex_jsonl\""),
        (CaptureSourceKind::PiJsonl, "\"pi_jsonl\""),
        (
            CaptureSourceKind::CursorCliAgentJsonl,
            "\"cursor_cli_agent_jsonl\"",
        ),
        (
            CaptureSourceKind::CursorIdeComposer,
            "\"cursor_ide_composer\"",
        ),
    ] {
        let native = SourceKind::from(kind);
        assert_eq!(serde_json::to_string(&native).unwrap(), wire);
        assert_eq!(serde_json::from_str::<SourceKind>(wire).unwrap(), native);
    }
    assert!(serde_json::from_str::<SourceKind>("\"unknown\"").is_err());
}
