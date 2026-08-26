// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
    pub const ALL: [Self; 9] = [
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
