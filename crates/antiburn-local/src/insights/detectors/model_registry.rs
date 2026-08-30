//! The compiled, reviewed model-replacement registry for the Obsolete
//! Model detector.
//!
//! Every rule below is a maintainer-reviewed judgment about when a
//! vendor's replacement model became available. Verified 2026-08-30
//! against these vendor announcements:
//!
//! - Claude Opus 5: <https://www.anthropic.com/news/claude-opus-5>
//!   (system card dated July 24, 2026). Released 2026-07-24.
//! - Claude Sonnet 5: <https://www.anthropic.com/news/claude-sonnet-5>.
//!   Released 2026-06-30.
//! - GPT-5.6: <https://openai.com/index/gpt-5-6/> (also reported by
//!   TechCrunch and Axios on 2026-07-09). Released 2026-07-09.
//!
//! The GPT-5.4 mini to GPT-5.6 Luna mapping is a judgment call: OpenAI
//! has not published an explicit size-tier successor for the mini
//! model. Treat it as provisional until a maintainer confirms it.

use std::collections::BTreeMap;

use crate::pricing::normalize_model_key;

/// This registry's revision. Bump it whenever a rule changes, so a
/// cached report can tell that its replacement facts are stale.
pub const REGISTRY_REVISION: u32 = 1;

/// One compiled rule: every source ID and alias it lists maps to the
/// same replacement, effective date, and rationale.
pub struct ModelReplacementRule {
    pub source_ids: &'static [&'static str],
    pub replacement: &'static str,
    pub available_since_ts_ms: i64,
    pub rationale: &'static str,
    pub source_url: &'static str,
}

/// One curated replacement entry for a deprecated model, keyed by
/// normalized model name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReplacementEntry {
    pub replacement: String,
    pub available_since_ts_ms: i64,
    pub rationale: String,
    pub source_url: String,
}

/// A compiled, reviewed replacement registry: every normalized source
/// ID and alias maps to its replacement entry, plus the registry's own
/// revision. An empty registry (the crate default before this rule
/// set shipped) can never prove absence of deprecated-model usage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelRegistry {
    pub revision: u32,
    pub entries: BTreeMap<String, ModelReplacementEntry>,
}

impl ModelRegistry {
    /// Builds a registry with no rules. `old_model_usage` treats this
    /// the same as any other empty registry: a contract gap, never
    /// clean.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Returns whether this registry carries no rules.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up one observed model, normalized with
    /// [`normalize_model_key`] and lowercased the same way the
    /// registry's own keys were built.
    pub fn lookup(&self, model: &str) -> Option<&ModelReplacementEntry> {
        let normalized = normalize_model_key(model).to_lowercase();
        self.entries.get(&normalized)
    }
}

/// Every reviewed rule this registry compiles. Add a rule here only
/// after a maintainer verifies the model IDs, replacement target, and
/// effective date against a vendor announcement (Decision, Open 1).
const RULES: &[ModelReplacementRule] = &[
    ModelReplacementRule {
        source_ids: &[
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-opus-4.6",
            "claude-opus-4.7",
            "claude-opus-4.8",
            "anthropic/claude-opus-4.8",
            "claude-opus-4-8-thinking-high",
        ],
        replacement: "claude-opus-5",
        // 2026-07-24T00:00:00Z
        available_since_ts_ms: 1_784_851_200_000,
        rationale: "Opus 5 succeeds the Opus 4.x line at the same tier.",
        source_url: "https://www.anthropic.com/news/claude-opus-5",
    },
    ModelReplacementRule {
        source_ids: &[
            "claude-sonnet-4-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4.6",
        ],
        replacement: "claude-sonnet-5",
        // 2026-06-30T00:00:00Z
        available_since_ts_ms: 1_782_777_600_000,
        rationale: "Sonnet 5 succeeds the Sonnet 4.x line at the same tier.",
        source_url: "https://www.anthropic.com/news/claude-sonnet-5",
    },
    ModelReplacementRule {
        source_ids: &["gpt-5.5", "gpt-5.5-fast", "gpt-5.4"],
        replacement: "gpt-5.6-sol",
        // 2026-07-09T00:00:00Z
        available_since_ts_ms: 1_783_555_200_000,
        rationale: "GPT-5.6 Sol succeeds GPT-5.4 and GPT-5.5 at the top capability tier.",
        source_url: "https://openai.com/index/gpt-5-6/",
    },
    ModelReplacementRule {
        source_ids: &["gpt-5.4-mini"],
        replacement: "gpt-5.6-luna",
        // 2026-07-09T00:00:00Z
        available_since_ts_ms: 1_783_555_200_000,
        rationale: "Judgment call: GPT-5.6 Luna is treated as GPT-5.4 mini's \
            size-tier successor. OpenAI has not published an explicit \
            mini-to-Luna mapping.",
        source_url: "https://openai.com/index/gpt-5-6/",
    },
];

/// Compiles [`RULES`] into the registry `ReportCatalogs::default` ships.
pub fn default_registry() -> ModelRegistry {
    let mut entries = BTreeMap::new();
    for rule in RULES {
        for source_id in rule.source_ids {
            let normalized = normalize_model_key(source_id).to_lowercase();
            entries.insert(
                normalized,
                ModelReplacementEntry {
                    replacement: rule.replacement.to_owned(),
                    available_since_ts_ms: rule.available_since_ts_ms,
                    rationale: rule.rationale.to_owned(),
                    source_url: rule.source_url.to_owned(),
                },
            );
        }
    }
    ModelRegistry {
        revision: REGISTRY_REVISION,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_key_normalizes_to_a_distinct_lookup() {
        let registry = default_registry();
        assert!(!registry.is_empty());
        for rule in RULES {
            for source_id in rule.source_ids {
                let entry = registry
                    .lookup(source_id)
                    .unwrap_or_else(|| panic!("{source_id} must resolve"));
                assert_eq!(entry.replacement, rule.replacement);
            }
        }
    }

    #[test]
    fn a_date_suffixed_source_id_resolves_through_normalization() {
        let registry = default_registry();
        let entry = registry
            .lookup("claude-opus-4-8-20260115")
            .expect("date-suffixed id must normalize onto the registry");
        assert_eq!(entry.replacement, "claude-opus-5");
    }

    #[test]
    fn a_mixed_case_source_id_resolves() {
        let registry = default_registry();
        let entry = registry
            .lookup("GPT-5.4")
            .expect("mixed-case gpt-5.4 must resolve to gpt-5.6-sol");
        assert_eq!(entry.replacement, "gpt-5.6-sol");
    }

    #[test]
    fn an_unlisted_model_does_not_resolve() {
        let registry = default_registry();
        assert!(registry.lookup("claude-haiku-4-5").is_none());
    }
}
