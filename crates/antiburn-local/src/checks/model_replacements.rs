//! The compiled, reviewed model-replacement registry for the Obsolete
//! Model detector.
//!
//! The reviewed registry is cross-checked against these vendor announcements:
//!
//! - Claude Opus 5: <https://www.anthropic.com/news/claude-opus-5>
//!   (system card dated July 24, 2026). Released 2026-07-24.
//! - Claude Sonnet 5: <https://www.anthropic.com/news/claude-sonnet-5>.
//!   Released 2026-06-30.
//! - GPT-5.6: <https://openai.com/index/gpt-5-6/> (also reported by
//!   TechCrunch and Axios on 2026-07-09). Released 2026-07-09.
//!
//! GPT-5.4 maps to GPT-5.6 Terra (the balanced tier), GPT-5.5 and
//! GPT-5.5 Fast map to GPT-5.6 Sol (the flagship tier), and GPT-5.4 mini
//! maps to GPT-5.6 Luna. OpenAI states these successor relationships;
//! they are not maintainer judgment calls.

use std::collections::BTreeMap;

use crate::pricing::canonical_model_key;

/// This registry's revision. Bump it whenever a rule changes, so a
/// cached report can tell that its replacement facts are stale.
pub const REGISTRY_REVISION: u32 = 2;

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

    /// Looks up one observed model, canonicalized with
    /// [`canonical_model_key`] the same way the registry's own keys were
    /// built.
    pub fn lookup(&self, model: &str) -> Option<&ModelReplacementEntry> {
        let canonical = canonical_model_key(model);
        self.entries.get(&canonical)
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
            // Canonicalizes (strip `antigravity-`, then
            // `normalize_model_key`) to `claude-opus-4-6-thinking`, which
            // is a distinct key from `claude-opus-4-6`; register it
            // explicitly so the alias resolves.
            "antigravity-claude-opus-4-6-thinking",
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
        source_ids: &["gpt-5.4"],
        replacement: "gpt-5.6-terra",
        // 2026-07-09T00:00:00Z
        available_since_ts_ms: 1_783_555_200_000,
        rationale: "GPT-5.6 Terra succeeds GPT-5.4 in the balanced capability-and-cost role.",
        source_url: "https://openai.com/index/gpt-5-6/",
    },
    ModelReplacementRule {
        source_ids: &["gpt-5.5", "gpt-5.5-fast"],
        replacement: "gpt-5.6-sol",
        // 2026-07-09T00:00:00Z
        available_since_ts_ms: 1_783_555_200_000,
        rationale: "GPT-5.6 Sol succeeds GPT-5.5 at the top capability tier.",
        source_url: "https://openai.com/index/gpt-5-6/",
    },
    ModelReplacementRule {
        source_ids: &["gpt-5.4-mini"],
        replacement: "gpt-5.6-luna",
        // 2026-07-09T00:00:00Z
        available_since_ts_ms: 1_783_555_200_000,
        rationale: "GPT-5.6 Luna is OpenAI's documented Codex replacement for \
            GPT-5.4 mini in efficient, high-volume work.",
        source_url: "https://openai.com/index/gpt-5-6/",
    },
];

/// Compiles [`RULES`] into the registry `ReportCatalogs::default` ships.
pub fn default_registry() -> ModelRegistry {
    let mut entries = BTreeMap::new();
    for rule in RULES {
        for source_id in rule.source_ids {
            let canonical = canonical_model_key(source_id);
            entries.insert(
                canonical,
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
            .expect("mixed-case gpt-5.4 must resolve to gpt-5.6-terra");
        assert_eq!(entry.replacement, "gpt-5.6-terra");
    }

    #[test]
    fn an_unlisted_model_does_not_resolve() {
        let registry = default_registry();
        assert!(registry.lookup("claude-haiku-4-5").is_none());
    }

    #[test]
    fn the_antigravity_opus_4_6_alias_resolves_to_opus_5() {
        let registry = default_registry();
        let entry = registry
            .lookup("antigravity-claude-opus-4-6-thinking")
            .expect("antigravity alias must resolve");
        assert_eq!(entry.replacement, "claude-opus-5");
    }

    #[test]
    fn gpt_5_4_resolves_to_terra_not_sol() {
        let registry = default_registry();
        let entry = registry.lookup("gpt-5.4").expect("gpt-5.4 must resolve");
        assert_eq!(entry.replacement, "gpt-5.6-terra");
    }

    #[test]
    fn gpt_5_5_resolves_to_sol() {
        let registry = default_registry();
        let entry = registry.lookup("gpt-5.5").expect("gpt-5.5 must resolve");
        assert_eq!(entry.replacement, "gpt-5.6-sol");
    }

    #[test]
    fn a_provider_namespaced_source_id_resolves_through_canonicalization() {
        let registry = default_registry();
        let entry = registry
            .lookup("openai.gpt-5.4")
            .expect("provider-prefixed gpt-5.4 must canonicalize and resolve");
        assert_eq!(entry.replacement, "gpt-5.6-terra");
    }
}
