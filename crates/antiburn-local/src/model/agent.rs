// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Canonical local agent identities.
//!
//! [`AgentKind`] names every coding agent the engine's v1 discovery matrix
//! supports, and nothing else. Slugs are the stable serialization used in
//! local persistence; display labels are the human-facing names.

use serde::{Deserialize, Serialize};

/// The coding agent that produced a local session.
///
/// Contains only the ratified v1 discovery agents. Serialization uses the
/// stable kebab-case slug (e.g. `"claude-code"`, `"amp-code"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    #[serde(rename = "claude-code")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "cursor")]
    Cursor,
    #[serde(rename = "copilot")]
    Copilot,
    #[serde(rename = "cline")]
    Cline,
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "kiro")]
    Kiro,
    #[serde(rename = "amp-code")]
    AmpCode,
    #[serde(rename = "antigravity")]
    Antigravity,
    #[serde(rename = "windsurf")]
    Windsurf,
    #[serde(rename = "pi")]
    Pi,
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.slug())
    }
}

impl AgentKind {
    /// Every supported agent. Iteration order is display order, not matcher
    /// precedence — discovery owns its own precedence list.
    pub const ALL: &'static [AgentKind] = &[
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::Copilot,
        AgentKind::Cline,
        AgentKind::OpenCode,
        AgentKind::Kiro,
        AgentKind::AmpCode,
        AgentKind::Antigravity,
        AgentKind::Windsurf,
        AgentKind::Pi,
    ];

    /// Stable kebab-case slug used for local serialization.
    pub fn slug(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
            AgentKind::Copilot => "copilot",
            AgentKind::Cline => "cline",
            AgentKind::OpenCode => "opencode",
            AgentKind::Kiro => "kiro",
            AgentKind::AmpCode => "amp-code",
            AgentKind::Antigravity => "antigravity",
            AgentKind::Windsurf => "windsurf",
            AgentKind::Pi => "pi",
        }
    }

    /// Parse the slug emitted by [`Self::slug`] back into an `AgentKind`.
    ///
    /// Returns `None` for unknown slugs (e.g. records written by a newer
    /// engine version).
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.slug() == slug)
    }

    /// Human-facing label for progress UI and logs (e.g. "Claude", "Amp").
    pub fn display_label(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
            AgentKind::Cursor => "Cursor",
            AgentKind::Copilot => "Copilot",
            AgentKind::Cline => "Cline",
            AgentKind::OpenCode => "OpenCode",
            AgentKind::Kiro => "Kiro",
            AgentKind::AmpCode => "Amp",
            AgentKind::Antigravity => "Antigravity",
            AgentKind::Windsurf => "Windsurf",
            AgentKind::Pi => "Pi",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentKind;

    #[test]
    fn slugs_round_trip() {
        for kind in AgentKind::ALL.iter().copied() {
            assert_eq!(AgentKind::from_slug(kind.slug()), Some(kind), "{kind:?}");
        }
        assert_eq!(AgentKind::from_slug("future-agent"), None);
    }

    #[test]
    fn serde_uses_slugs() {
        let json = serde_json::to_value(AgentKind::Claude).unwrap();
        assert_eq!(json, "claude-code");
        let json = serde_json::to_value(AgentKind::AmpCode).unwrap();
        assert_eq!(json, "amp-code");
        let back: AgentKind = serde_json::from_value(serde_json::json!("opencode")).unwrap();
        assert_eq!(back, AgentKind::OpenCode);
    }

    #[test]
    fn display_matches_slug() {
        for kind in AgentKind::ALL.iter().copied() {
            assert_eq!(kind.to_string(), kind.slug());
        }
    }

    #[test]
    fn all_slugs_unique() {
        let mut slugs: Vec<&str> = AgentKind::ALL.iter().map(|kind| kind.slug()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), AgentKind::ALL.len());
    }
}
