// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local-only skill-invocation details.
//!
//! A "skill" is a Claude-Code–style `Skill` tool call (and the equivalent
//! `skill` tool other agents emit). The analysis engine detects invocations
//! in a transcript ([`SkillUse`]); the app may enrich each one with details
//! read from the invoked skill's local `SKILL.md` ([`LocalSkillDetails`]).
//! The combined [`SkillDetail`] is rendered only in the local UI — nothing
//! here is a wire contract.
//!
//! ## Serialized shape
//!
//! [`SkillDetail`] flattens [`SkillUse`], so its JSON is a single flat object
//! (`{ name, progress, durationMs, …, local: { … } }`). serde's
//! `deny_unknown_fields` is applied to the leaf [`LocalSkillDetails`] rather
//! than to [`SkillDetail`] — `deny_unknown_fields` is documented as
//! incompatible with `flatten`.

use serde::{Deserialize, Serialize};

/// One skill invocation extracted from a transcript, vendor-neutral.
///
/// Produced by the analysis engine from a `Skill` tool call. Flattened into
/// [`SkillDetail`] for the UI shape, so its fields sit at the top level of
/// each skill object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUse {
    /// The invoked skill's name (from the tool input; `"skill"` when unresolved).
    pub name: String,
    /// 0..1 position on the session's active-time axis (the hypnogram x-axis).
    pub progress: f32,
    /// One-line description grafted from the raw transcript's skill listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Idle-capped gap to the next event (ms); `None` when there are no timestamps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Output tokens of the invoking turn.
    pub tokens_out: u64,
    /// Context-window occupancy at the invoking turn.
    pub context_tokens: u64,
}

/// Where an invoked skill's `SKILL.md` was resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillScope {
    /// A user-global install (e.g. `~/.claude/skills/<name>/SKILL.md`).
    Global,
    /// A repo-local install (e.g. `<repo>/.claude/skills/<name>/SKILL.md`).
    Project,
    /// A skill bundled inside an installed plugin.
    Plugin,
    /// Scope could not be determined (or the skill could not be resolved).
    #[default]
    Unknown,
}

/// Locally-extracted `SKILL.md` details.
///
/// Every field is best-effort: a resolvable but partially-specified
/// `SKILL.md` fills only the fields it declares. When the skill can't be
/// resolved at all the whole struct is omitted
/// (`SkillDetail::local == None`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalSkillDetails {
    /// Frontmatter `version:`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Authoritative description, from the `SKILL.md` frontmatter `description:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared tools, from frontmatter `allowed-tools:`/`tools:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Frontmatter `license:`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Where the `SKILL.md` was resolved from (project beats global).
    pub scope: SkillScope,
    /// Absolute path to the resolved `SKILL.md`, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `SKILL.md` mtime as epoch milliseconds, when stat succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<i64>,
    /// `SKILL.md` size in bytes, when stat succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Whether the `SKILL.md` was found and read on disk.
    pub available: bool,
}

/// A skill invocation enriched with its local `SKILL.md` details.
///
/// [`SkillUse`] is flattened, so the serialized object carries the usage
/// fields at the top level alongside the optional `local` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    /// The transcript-derived invocation, flattened to the top level.
    #[serde(flatten)]
    pub usage: SkillUse,
    /// Locally-read `SKILL.md` enrichment; `None` when the skill couldn't be
    /// resolved on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalSkillDetails>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_use_serializes_camel_case_and_skips_none() {
        let usage = SkillUse {
            name: "deep-research".to_string(),
            progress: 0.42,
            description: None,
            duration_ms: None,
            tokens_out: 1234,
            context_tokens: 56_000,
        };
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["name"], "deep-research");
        assert_eq!(json["tokensOut"], 1234);
        assert_eq!(json["contextTokens"], 56_000);
        // None Options are omitted.
        assert!(json.get("description").is_none());
        assert!(json.get("durationMs").is_none());
    }

    #[test]
    fn skill_detail_flattens_usage_at_top_level() {
        let detail = SkillDetail {
            usage: SkillUse {
                name: "checkpoint".to_string(),
                progress: 0.1,
                description: Some("Checkpoint command".to_string()),
                duration_ms: Some(2000),
                tokens_out: 10,
                context_tokens: 20,
            },
            local: Some(LocalSkillDetails {
                version: Some("1.2.3".to_string()),
                description: Some("Authoritative description".to_string()),
                allowed_tools: vec!["Read".to_string(), "Edit".to_string()],
                license: None,
                scope: SkillScope::Project,
                path: Some("/home/x/.claude/skills/checkpoint/SKILL.md".to_string()),
                modified_at_ms: Some(1_700_000_000_000),
                size_bytes: Some(512),
                available: true,
            }),
        };
        let json = serde_json::to_value(&detail).unwrap();
        // Flattened usage fields live at the top level, not under `usage`.
        assert!(json.get("usage").is_none());
        assert_eq!(json["name"], "checkpoint");
        assert_eq!(json["durationMs"], 2000);
        // Nested local block uses camelCase too.
        assert_eq!(json["local"]["scope"], "project");
        assert_eq!(json["local"]["allowedTools"][0], "Read");

        // Round-trips.
        let back: SkillDetail = serde_json::from_value(json).unwrap();
        assert_eq!(back, detail);
    }

    #[test]
    fn local_details_reject_unknown_fields() {
        let json = serde_json::json!({
            "scope": "global",
            "available": true,
            "bogusField": 1
        });
        let parsed: Result<LocalSkillDetails, _> = serde_json::from_value(json);
        assert!(parsed.is_err(), "unknown fields must be rejected");
    }

    #[test]
    fn skill_scope_defaults_to_unknown() {
        assert_eq!(SkillScope::default(), SkillScope::Unknown);
        let json = serde_json::to_value(SkillScope::Unknown).unwrap();
        assert_eq!(json, "unknown");
    }
}
