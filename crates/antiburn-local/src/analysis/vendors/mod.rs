// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Vendor adapter registry — the dispatch half of the interface layer.
//!
//! [`adapter_for`] maps a vendor label to its [`VendorAdapter`]. Every label,
//! known or not, resolves to *some* adapter (generic JSONL by default), so no
//! vendor is ever silently dropped from analysis.

mod antigravity;
mod claude;
mod codex;
mod cursor;
mod generic_jsonl;
mod jsonl;
mod opencode;
mod sqlite;

use crate::analysis::interface::{RawSource, VendorAdapter};

static CLAUDE: claude::ClaudeAdapter = claude::ClaudeAdapter;
static GENERIC: generic_jsonl::GenericJsonlAdapter = generic_jsonl::GenericJsonlAdapter;
static CODEX: codex::CodexAdapter = codex::CodexAdapter;
static CURSOR: cursor::CursorAdapter = cursor::CursorAdapter;
static OPENCODE: opencode::OpenCodeAdapter = opencode::OpenCodeAdapter;
static ANTIGRAVITY: antigravity::AntigravityAdapter = antigravity::AntigravityAdapter;

/// Resolve the adapter for a vendor label (case-insensitive).
pub fn adapter_for(agent: &str) -> &'static dyn VendorAdapter {
    match agent.to_ascii_lowercase().as_str() {
        "claude" => &CLAUDE,
        "codex" => &CODEX,
        "cursor" => &CURSOR,
        "opencode" => &OPENCODE,
        "antigravity" => &ANTIGRAVITY,
        _ => &GENERIC,
    }
}

/// Whether a vendor label has a dedicated (non-generic) adapter — i.e. it
/// resolves to one of the bespoke arms of [`adapter_for`] rather than the
/// generic JSONL fallback. Derived from `adapter_for` itself so there's no
/// second vendor list to keep in sync: adding a bespoke adapter there
/// automatically extends this. Mirrors the frontend's `agentSupportsAnalytics`
/// allowlist. Case-insensitive (via `adapter_for`).
///
/// Used to scope features that should only run for vendors we model precisely
/// (e.g. the background health/drift signal), so a generically-parsed session
/// never produces a half-confident metric.
pub fn has_dedicated_adapter(agent: &str) -> bool {
    adapter_for(agent).agent() != GENERIC.agent()
}

/// Read a non-SQLite source into a string. SQLite sources are handled directly
/// by the SQLite adapter and must not be routed here.
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
    fn dedicated_adapters_are_recognized_case_insensitively() {
        for agent in ["claude", "codex", "cursor", "opencode", "antigravity"] {
            assert!(has_dedicated_adapter(agent));
            assert!(has_dedicated_adapter(&agent.to_uppercase()));
        }
    }

    #[test]
    fn generic_fallback_vendors_have_no_dedicated_adapter() {
        // Vendors on the generic JSONL fallback and unknown slugs return false,
        // so features keyed on this predicate leave them untouched.
        for agent in ["copilot", "cline", "windsurf", "pi", "", "totally-unknown"] {
            assert!(!has_dedicated_adapter(agent));
        }
    }
}
