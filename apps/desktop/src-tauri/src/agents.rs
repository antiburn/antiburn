//! Translating between the engine's two names for an agent.
//!
//! The engine names an agent twice, for two different jobs:
//!
//! - [`AgentKind::slug`] is the **discovery/serialization** slug
//!   (`"claude-code"`, `"amp-code"`). It is what the store persists and what
//!   the webview's agent registry keys off.
//! - `analysis::reader_for` dispatches on a **vendor label** (`"claude"`,
//!   `"codex"`, …), which is not the same string: `reader_for("claude-code")`
//!   falls through to the generic JSONL adapter and would silently produce a
//!   worse analysis for the app's most common agent.
//!
//! Nothing in the engine bridges the two, so the shell does it here, in one
//! place, with a test that pins every kind whose label differs from its slug.

use antiburn_local::analysis::has_dedicated_reader;
use antiburn_local::model::AgentKind;

/// The vendor label `analysis::reader_for` dispatches on for `kind`.
///
/// Kinds whose vendor label matches their slug fall through to the slug, so a
/// future agent needs an arm here only when the two names diverge.
pub fn vendor_label(kind: AgentKind) -> &'static str {
    match kind {
        // The one real divergence: the slug carries the product name, the
        // adapter registry the vendor name.
        AgentKind::Claude => "claude",
        other => other.slug(),
    }
}

/// Whether the engine has a usable session parser for this agent.
/// Passive source registration does not enable analysis.
///
/// Mirrors the webview's `agentSupportsAnalysis`, but asks the engine rather
/// than a second hand-maintained list.
pub fn supports_analysis(kind: AgentKind) -> bool {
    has_dedicated_reader(vendor_label(kind))
}

/// Parse the slug the store persists back into an [`AgentKind`].
pub fn kind_from_slug(slug: &str) -> Option<AgentKind> {
    AgentKind::from_slug(slug)
}

/// Return the agents that use the durable evidence queue.
///
/// Every supported [`AgentKind`] is in the cohort, so this can never drift
/// from the engine's discovery matrix.
pub fn evidence_cohort() -> Vec<&'static str> {
    AgentKind::ALL.iter().map(|kind| kind.slug()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_resolves_to_a_vendor_label_the_registry_recognizes() {
        for kind in AgentKind::ALL.iter().copied() {
            assert_eq!(
                antiburn_local::analysis::reader_for(vendor_label(kind)).agent(),
                vendor_label(kind)
            );
        }
    }

    #[test]
    fn only_functional_session_parsers_support_analysis() {
        for kind in AgentKind::ALL.iter().copied() {
            let expected = matches!(
                kind,
                AgentKind::Claude
                    | AgentKind::Codex
                    | AgentKind::Cursor
                    | AgentKind::OpenCode
                    | AgentKind::Pi
                    | AgentKind::Antigravity
            );
            assert_eq!(supports_analysis(kind), expected, "{kind:?}");
        }
    }

    #[test]
    fn the_claude_slug_would_miss_its_adapter_without_the_mapping() {
        // This is the whole reason the module exists; if the engine ever aligns
        // the two names, this assertion is what tells us the mapping is dead.
        assert_eq!(AgentKind::Claude.slug(), "claude-code");
        assert_eq!(vendor_label(AgentKind::Claude), "claude");
        assert!(!has_dedicated_reader(AgentKind::Claude.slug()));
        assert!(has_dedicated_reader(vendor_label(AgentKind::Claude)));
    }

    #[test]
    fn the_evidence_cohort_uses_the_discovery_slug() {
        assert_eq!(
            evidence_cohort(),
            vec![
                "claude-code",
                "codex",
                "cursor",
                "copilot",
                "cline",
                "opencode",
                "kiro",
                "amp-code",
                "antigravity",
                "windsurf",
                "pi",
            ]
        );
        assert_eq!(evidence_cohort().len(), AgentKind::ALL.len());
        assert_eq!(AgentKind::Codex.slug(), vendor_label(AgentKind::Codex));
        assert_eq!(
            AgentKind::OpenCode.slug(),
            vendor_label(AgentKind::OpenCode)
        );
        assert_eq!(AgentKind::Pi.slug(), vendor_label(AgentKind::Pi));
    }

    #[test]
    fn slugs_round_trip_through_the_store_spelling() {
        for kind in AgentKind::ALL.iter().copied() {
            assert_eq!(kind_from_slug(kind.slug()), Some(kind));
        }
        assert_eq!(kind_from_slug("not-an-agent"), None);
    }
}
