//! Report-time model and detector policy catalogs.

use std::collections::{BTreeMap, BTreeSet};

use crate::pricing::canonical_model_key;

use super::model_replacements::{ModelRegistry, default_registry};

/// A model family, derived from the normalized model key's prefix.
/// Tier policy is keyed by family, not by harness, because OpenCode and
/// Pi can run any vendor's models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelFamily {
    Claude,
    OpenAi,
    Google,
    /// No known vendor prefix matched. Tier checks report a contract gap.
    Unknown,
}

/// Classifies a model key from its canonical provider-neutral prefix.
pub fn model_family(model: &str) -> ModelFamily {
    let canonical = canonical_model_key(model);
    if canonical.starts_with("claude-") {
        ModelFamily::Claude
    } else if canonical.starts_with("gpt-")
        || canonical.starts_with("o1")
        || canonical.starts_with("o3")
        || canonical.starts_with("o4")
    {
        ModelFamily::OpenAi
    } else if canonical.starts_with("gemini-") {
        ModelFamily::Google
    } else {
        ModelFamily::Unknown
    }
}

/// One family's recognized reasoning-effort labels and labels above its cap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffortPolicy {
    pub above_cap: BTreeSet<String>,
    pub recognized: BTreeSet<String>,
}

/// The normalized speed labels recognized for one model family.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeedPolicy {
    pub recognized: BTreeSet<String>,
}

/// Premium-tier classification for one model family.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PremiumPolicy {
    pub reviewed: bool,
    /// A canonical key is premium when it contains any listed substring.
    pub substrings: Vec<String>,
    /// A canonical key is premium when it starts with any listed prefix.
    pub prefixes: Vec<String>,
    /// Canonical keys that override a matching substring or prefix.
    pub exceptions: BTreeSet<String>,
}
impl PremiumPolicy {
    /// Returns whether a canonical model key is premium under this policy.
    pub fn is_premium(&self, canonical: &str) -> bool {
        if self.exceptions.contains(canonical) {
            return false;
        }
        self.substrings
            .iter()
            .any(|s| canonical.contains(s.as_str()))
            || self
                .prefixes
                .iter()
                .any(|p| canonical.starts_with(p.as_str()))
    }
}

/// The complete tier and cache policy for one model family.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FamilyPolicy {
    pub effort: EffortPolicy,
    pub speed: SpeedPolicy,
    pub premium: PremiumPolicy,
    /// Whether Cache Churn has a reviewed threshold for this family.
    pub cache_policy_reviewed: bool,
    /// The overpay multiple at or above which Cache Churn finds a problem.
    pub cache_overpay_multiple_threshold: f64,
}

/// Report-time policy inputs. Catalog changes do not require transcript
/// parsing or changes to persisted evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportCatalogs {
    pub revision: i64,
    /// A request above this observed context depth is a finding.
    pub depth_cap_tokens: u64,
    /// Reviewed policy, with one entry per model family.
    pub families: BTreeMap<ModelFamily, FamilyPolicy>,
    /// Curated deprecated-model registry.
    pub model_replacements: ModelRegistry,
    /// Delegated fast-tier turns at or above this count are a finding.
    pub fast_mode_delegated_turns_threshold: u64,
}

/// Effort labels above the recommended cap in every reviewed family.
fn above_cap_effort_tiers() -> BTreeSet<String> {
    ["xhigh", "max", "ultra"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

impl Default for ReportCatalogs {
    fn default() -> Self {
        let mut families = BTreeMap::new();
        families.insert(
            ModelFamily::Claude,
            FamilyPolicy {
                effort: EffortPolicy {
                    above_cap: above_cap_effort_tiers(),
                    recognized: ["low", "medium", "high"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain(above_cap_effort_tiers())
                        .collect(),
                },
                speed: SpeedPolicy {
                    recognized: ["fast", "standard"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                },
                premium: PremiumPolicy {
                    reviewed: true,
                    substrings: ["opus", "fable", "mythos"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    prefixes: Vec::new(),
                    exceptions: BTreeSet::new(),
                },
                cache_policy_reviewed: true,
                cache_overpay_multiple_threshold: 2.35,
            },
        );
        families.insert(
            ModelFamily::OpenAi,
            FamilyPolicy {
                effort: EffortPolicy {
                    above_cap: above_cap_effort_tiers(),
                    recognized: ["none", "minimal", "low", "medium", "high"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain(above_cap_effort_tiers())
                        .collect(),
                },
                speed: SpeedPolicy {
                    recognized: ["fast", "standard"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                },
                premium: PremiumPolicy {
                    reviewed: true,
                    substrings: Vec::new(),
                    prefixes: vec![
                        "gpt-6-astra".to_owned(),
                        "gpt-5.6".to_owned(),
                        "gpt-5.5".to_owned(),
                    ],
                    exceptions: [
                        "gpt-5.6-terra",
                        "gpt-5.6-luna",
                        "gpt-5.3-codex-spark",
                        "codex-auto-review",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                },
                cache_policy_reviewed: true,
                cache_overpay_multiple_threshold: 2.0,
            },
        );
        families.insert(
            ModelFamily::Google,
            FamilyPolicy {
                premium: PremiumPolicy {
                    reviewed: true,
                    substrings: vec!["pro".to_owned()],
                    prefixes: Vec::new(),
                    exceptions: BTreeSet::new(),
                },
                cache_policy_reviewed: false,
                ..FamilyPolicy::default()
            },
        );
        families.insert(ModelFamily::Unknown, FamilyPolicy::default());
        Self {
            revision: 8,
            depth_cap_tokens: 400_000,
            families,
            model_replacements: default_registry(),
            fast_mode_delegated_turns_threshold: 1,
        }
    }
}
