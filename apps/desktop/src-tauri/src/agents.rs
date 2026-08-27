//! Translating between the engine's two names for an agent.
//!
//! The engine names an agent twice, for two different jobs:
//!
//! - [`AgentKind::slug`] is the **discovery/serialization** slug
//!   (`"claude-code"`, `"amp-code"`). It is what the store persists and what
//!   the webview's agent registry keys off.
//! - `analysis::adapter_for` dispatches on a **vendor label** (`"claude"`,
//!   `"codex"`, …), which is not the same string: `adapter_for("claude-code")`
//!   falls through to the generic JSONL adapter and would silently produce a
//!   worse analysis for the app's most common agent.
//!
//! Nothing in the engine bridges the two, so the shell does it here, in one
//! place, with a test that pins every kind whose label differs from its slug.

use antiburn_local::analysis::has_dedicated_adapter;
use antiburn_local::model::AgentKind;

/// The vendor label `analysis::adapter_for` dispatches on for `kind`.
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

/// Whether the engine has a dedicated adapter for this agent's transcript
/// format, as opposed to the generic JSONL fallback.
///
/// Mirrors the webview's `agentSupportsAnalysis`, but asks the engine rather
/// than a second hand-maintained list.
pub fn supports_analysis(kind: AgentKind) -> bool {
    has_dedicated_adapter(vendor_label(kind))
}

/// Parse the slug the store persists back into an [`AgentKind`].
pub fn kind_from_slug(slug: &str) -> Option<AgentKind> {
    AgentKind::from_slug(slug)
}

/// Return the agents that use the durable evidence queue.
pub fn evidence_cohort() -> [&'static str; 3] {
    [
        AgentKind::Claude.slug(),
        AgentKind::Codex.slug(),
        AgentKind::Pi.slug(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_resolves_to_a_vendor_label_the_registry_recognizes() {
        // The registry answers for every label (unknown ones get the generic
        // adapter), so the meaningful assertion is that the kinds the engine
        // models precisely actually reach their dedicated adapter.
        let dedicated = [
            AgentKind::Claude,
            AgentKind::Codex,
            AgentKind::Cursor,
            AgentKind::OpenCode,
            AgentKind::Pi,
            AgentKind::Antigravity,
        ];
        for kind in dedicated {
            assert!(
                supports_analysis(kind),
                "{kind:?} should reach its dedicated adapter via {:?}",
                vendor_label(kind)
            );
        }
    }

    #[test]
    fn the_claude_slug_would_miss_its_adapter_without_the_mapping() {
        // This is the whole reason the module exists; if the engine ever aligns
        // the two names, this assertion is what tells us the mapping is dead.
        assert_eq!(AgentKind::Claude.slug(), "claude-code");
        assert_eq!(vendor_label(AgentKind::Claude), "claude");
        assert!(!has_dedicated_adapter(AgentKind::Claude.slug()));
        assert!(has_dedicated_adapter(vendor_label(AgentKind::Claude)));
    }

    #[test]
    fn generic_fallback_agents_report_no_dedicated_adapter() {
        for kind in [
            AgentKind::Copilot,
            AgentKind::Cline,
            AgentKind::Kiro,
            AgentKind::AmpCode,
            AgentKind::Windsurf,
        ] {
            assert!(!supports_analysis(kind), "{kind:?}");
        }
    }

    #[test]
    fn the_evidence_cohort_uses_the_discovery_slug() {
        assert_eq!(
            evidence_cohort(),
            [
                AgentKind::Claude.slug(),
                AgentKind::Codex.slug(),
                AgentKind::Pi.slug(),
            ]
        );
        assert_eq!(AgentKind::Codex.slug(), vendor_label(AgentKind::Codex));
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
