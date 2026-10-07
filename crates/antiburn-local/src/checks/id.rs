/// Identifies one provider-neutral report detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DetectorId {
    SessionsOverDepth,
    ModelOverthinking,
    OverpoweredSubagents,
    UnusedMcpServers,
    UnusedBuiltInTools,
    UnusedSkills,
    OldModelUsage,
    OveruseOfFastMode,
    CacheChurn,
    IgnoredInstructions,
    SkillOpportunities,
    OverExploring,
    ScopeCreep,
}

impl DetectorId {
    pub const COUNT: usize = 13;

    pub const ALL: [Self; Self::COUNT] = [
        Self::SessionsOverDepth,
        Self::ModelOverthinking,
        Self::OverpoweredSubagents,
        Self::UnusedMcpServers,
        Self::UnusedBuiltInTools,
        Self::UnusedSkills,
        Self::OldModelUsage,
        Self::OveruseOfFastMode,
        Self::CacheChurn,
        Self::IgnoredInstructions,
        Self::SkillOpportunities,
        Self::OverExploring,
        Self::ScopeCreep,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    /// Returns the stable serialized key for this detector.
    pub const fn key(self) -> &'static str {
        match self {
            Self::SessionsOverDepth => "sessions_over_depth",
            Self::ModelOverthinking => "model_overthinking",
            Self::OverpoweredSubagents => "overpowered_subagents",
            Self::UnusedMcpServers => "unused_mcp_servers",
            Self::UnusedBuiltInTools => "unused_built_in_tools",
            Self::UnusedSkills => "unused_skills",
            Self::OldModelUsage => "old_model_usage",
            Self::OveruseOfFastMode => "overuse_of_fast_mode",
            Self::CacheChurn => "cache_churn",
            Self::IgnoredInstructions => "ignored_instructions",
            Self::SkillOpportunities => "skill_opportunities",
            Self::OverExploring => "over_exploring",
            Self::ScopeCreep => "scope_creep",
        }
    }

    /// Parses a stable serialized detector key.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "sessions_over_depth" => Some(Self::SessionsOverDepth),
            "model_overthinking" => Some(Self::ModelOverthinking),
            "overpowered_subagents" => Some(Self::OverpoweredSubagents),
            "unused_mcp_servers" => Some(Self::UnusedMcpServers),
            "unused_built_in_tools" => Some(Self::UnusedBuiltInTools),
            "unused_skills" => Some(Self::UnusedSkills),
            "old_model_usage" => Some(Self::OldModelUsage),
            "overuse_of_fast_mode" => Some(Self::OveruseOfFastMode),
            "cache_churn" => Some(Self::CacheChurn),
            "ignored_instructions" => Some(Self::IgnoredInstructions),
            "skill_opportunities" => Some(Self::SkillOpportunities),
            "over_exploring" => Some(Self::OverExploring),
            "scope_creep" => Some(Self::ScopeCreep),
            _ => None,
        }
    }
}

/// Selects the detectors that may run during one report reduction.
///
/// Engine callers use [`Self::all`] by default. Product surfaces can pass a
/// narrower snapshot without changing parsing, indexing, or shared evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorSelection {
    enabled: [bool; DetectorId::COUNT],
}

impl DetectorSelection {
    pub const fn all() -> Self {
        Self {
            enabled: [true; DetectorId::COUNT],
        }
    }

    pub const fn none() -> Self {
        Self {
            enabled: [false; DetectorId::COUNT],
        }
    }

    pub fn from_enabled(detectors: impl IntoIterator<Item = DetectorId>) -> Self {
        let mut selection = Self::none();
        for detector in detectors {
            selection.enabled[detector.index()] = true;
        }
        selection
    }

    pub const fn contains(&self, detector: DetectorId) -> bool {
        self.enabled[detector.index()]
    }

    pub fn iter(&self) -> impl Iterator<Item = DetectorId> + '_ {
        DetectorId::ALL
            .into_iter()
            .filter(|detector| self.contains(*detector))
    }
}

impl Default for DetectorSelection {
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{DetectorId, DetectorSelection};

    #[test]
    fn detector_keys_are_unique_and_round_trip() {
        let keys: BTreeSet<_> = DetectorId::ALL.into_iter().map(DetectorId::key).collect();

        assert_eq!(DetectorId::ALL.len(), DetectorId::COUNT);
        assert_eq!(keys.len(), DetectorId::COUNT);
        for detector in DetectorId::ALL {
            assert_eq!(DetectorId::from_key(detector.key()), Some(detector));
        }
        assert_eq!(DetectorId::from_key("unknown_detector"), None);
    }

    #[test]
    fn detector_order_and_mask_bits_are_stable() {
        assert_eq!(
            DetectorId::ALL,
            [
                DetectorId::SessionsOverDepth,
                DetectorId::ModelOverthinking,
                DetectorId::OverpoweredSubagents,
                DetectorId::UnusedMcpServers,
                DetectorId::UnusedBuiltInTools,
                DetectorId::UnusedSkills,
                DetectorId::OldModelUsage,
                DetectorId::OveruseOfFastMode,
                DetectorId::CacheChurn,
                DetectorId::IgnoredInstructions,
                DetectorId::SkillOpportunities,
                DetectorId::OverExploring,
                DetectorId::ScopeCreep,
            ]
        );
        let mask = DetectorId::ALL
            .into_iter()
            .fold(0u16, |mask, detector| mask | (1 << detector.index()));
        assert_eq!(mask, 0b1_1111_1111_1111);
        for detector in DetectorId::ALL {
            assert_eq!(1u16 << detector.index(), mask & (1u16 << detector.index()));
        }
    }

    #[test]
    fn detector_selection_supports_all_none_and_subsets() {
        assert_eq!(DetectorSelection::all().iter().count(), DetectorId::COUNT);
        assert_eq!(DetectorSelection::none().iter().count(), 0);

        let selection = DetectorSelection::from_enabled([
            DetectorId::SessionsOverDepth,
            DetectorId::UnusedSkills,
        ]);
        assert!(selection.contains(DetectorId::SessionsOverDepth));
        assert!(selection.contains(DetectorId::UnusedSkills));
        assert!(!selection.contains(DetectorId::IgnoredInstructions));
        assert_eq!(selection.iter().count(), 2);
    }
}
