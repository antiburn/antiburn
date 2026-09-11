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
}

impl DetectorId {
    pub const COUNT: usize = 9;

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
            _ => None,
        }
    }
}

/// Identifies one exclusive coverage bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageBucket {
    UnknownStart,
    Pending,
    Processing,
    Failed,
    Unsupported,
    Stale,
    Ready,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::DetectorId;

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
}
