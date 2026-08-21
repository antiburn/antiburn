// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Skill invocations extracted from session transcripts.

use serde::{Deserialize, Serialize};

/// One skill invocation extracted from a transcript, vendor-neutral.
///
/// The analysis engine produces this value from a `Skill` tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUse {
    /// The invoked skill's name (from the tool input; `"skill"` when unresolved).
    pub name: String,
    /// The invocation position on the session's active-time axis, from 0 to 1.
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
}
