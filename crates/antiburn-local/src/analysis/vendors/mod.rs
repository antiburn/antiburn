//! Session reader registry for native source dispatch.
//!
//! [`reader_for`] maps a vendor label to its [`SessionReader`]. Every label,
//! known or not, resolves to *some* reader (generic JSONL by default), so no
//! vendor is ever silently dropped from analysis.

mod antigravity;
pub mod claude;
mod codex;
mod cursor;
mod generic_jsonl;
mod opencode;
mod passive;
pub(crate) mod pi;

use crate::analysis::interface::{RawSource, SessionReader};

static CLAUDE: claude::ClaudeSessionReader = claude::ClaudeSessionReader;
static GENERIC: generic_jsonl::GenericJsonlSessionReader = generic_jsonl::GenericJsonlSessionReader;
static CODEX: codex::CodexSessionReader = codex::CodexSessionReader;
static CURSOR: cursor::CursorSessionReader = cursor::CursorSessionReader;
static OPENCODE: opencode::OpenCodeSessionReader = opencode::OpenCodeSessionReader;
static PI: pi::PiSessionReader = pi::PiSessionReader;
static ANTIGRAVITY: antigravity::AntigravitySessionReader = antigravity::AntigravitySessionReader;
static COPILOT: passive::PassiveSessionReader = passive::PassiveSessionReader {
    agent: "copilot",
    format: crate::analysis::SourceFormat::CopilotCliJsonl,
};
static CLINE: passive::PassiveSessionReader = passive::PassiveSessionReader {
    agent: "cline",
    format: crate::analysis::SourceFormat::ClineSessionJson,
};
static KIRO: passive::PassiveSessionReader = passive::PassiveSessionReader {
    agent: "kiro",
    format: crate::analysis::SourceFormat::KiroSessionJson,
};
static AMP: passive::PassiveSessionReader = passive::PassiveSessionReader {
    agent: "amp-code",
    format: crate::analysis::SourceFormat::AmpThreadJson,
};
static WINDSURF: passive::PassiveSessionReader = passive::PassiveSessionReader {
    agent: "windsurf",
    format: crate::analysis::SourceFormat::WindsurfWorkspaceJson,
};

/// Resolve the reader for an agent label, without case sensitivity.
pub fn reader_for(agent: &str) -> &'static dyn SessionReader {
    match agent.to_ascii_lowercase().as_str() {
        "claude" => &CLAUDE,
        "codex" => &CODEX,
        "cursor" => &CURSOR,
        "copilot" => &COPILOT,
        "cline" => &CLINE,
        "opencode" => &OPENCODE,
        "kiro" => &KIRO,
        "amp-code" => &AMP,
        "pi" => &PI,
        "antigravity" => &ANTIGRAVITY,
        "windsurf" => &WINDSURF,
        _ => &GENERIC,
    }
}

/// Whether an agent label has a dedicated reader instead of the generic reader.
///
/// Used to scope features that should only run for vendors we model precisely
/// (e.g. the background health/drift signal), so a generically-parsed session
/// never produces a half-confident metric.
pub fn has_dedicated_reader(agent: &str) -> bool {
    reader_for(agent).agent() != GENERIC.agent()
}

/// Read a non-SQLite source into a string. SQLite sources are handled directly
/// by the SQLite reader and must not be routed here.
pub(crate) fn read_source(source: &RawSource) -> anyhow::Result<std::borrow::Cow<'_, str>> {
    match source {
        RawSource::Jsonl(content) => Ok(std::borrow::Cow::Borrowed(content)),
        RawSource::File(path) => Ok(std::borrow::Cow::Owned(std::fs::read_to_string(path)?)),
        RawSource::Sqlite(path) => {
            anyhow::bail!(
                "sqlite source must be handled by the sqlite adapter: {}",
                path.display()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_source_is_borrowed_without_copying() {
        let source = RawSource::Jsonl("large session body".to_string());
        assert!(matches!(
            read_source(&source).unwrap(),
            std::borrow::Cow::Borrowed("large session body")
        ));
    }

    #[test]
    fn dedicated_readers_are_recognized_case_insensitively() {
        for agent in [
            "claude",
            "codex",
            "cursor",
            "copilot",
            "cline",
            "opencode",
            "kiro",
            "amp-code",
            "pi",
            "antigravity",
            "windsurf",
        ] {
            assert!(has_dedicated_reader(agent));
            assert!(has_dedicated_reader(&agent.to_uppercase()));
        }
    }

    #[test]
    fn unknown_agents_have_no_dedicated_reader() {
        for agent in ["", "totally-unknown"] {
            assert!(!has_dedicated_reader(agent));
        }
    }

    #[test]
    fn passive_readers_keep_agent_specific_source_formats() {
        let cases = [
            ("copilot", crate::analysis::SourceFormat::CopilotCliJsonl),
            ("cline", crate::analysis::SourceFormat::ClineSessionJson),
            ("kiro", crate::analysis::SourceFormat::KiroSessionJson),
            ("amp-code", crate::analysis::SourceFormat::AmpThreadJson),
            (
                "windsurf",
                crate::analysis::SourceFormat::WindsurfWorkspaceJson,
            ),
        ];
        for (agent, expected) in cases {
            let capabilities = reader_for(agent).capabilities(&RawSource::Jsonl(String::new()));
            assert_eq!(capabilities.source_format, expected, "{agent}");
            assert_eq!(
                capabilities,
                crate::analysis::SourceCapabilities::uncharacterized(expected),
                "{agent} must fail closed"
            );
        }
    }
}
