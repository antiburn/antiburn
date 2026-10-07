//! Pure investigation assessment. Source adapters supply recorded evidence and
//! explicit episode boundaries. This module does not read files or call a model.

mod assessment;
mod episodes;
mod questions;

pub use assessment::{
    Abstention, Assessment, Decision, EvidenceJudgments, OverExploringCheck, PreparedAssessment,
    ReadBinding, Reason, SEMANTIC_PROBABILITY_THRESHOLD, SamplingCandidate, SemanticOutcome,
    Target, Unassessed, WITHIN_FILE_SUBSTANTIAL_PROBABILITY_THRESHOLD, synchronize_sampling,
};
pub use episodes::{
    EpisodeSpan, EpisodeState, InvestigationEpisode, OverExploringInput, ReadObservation,
    build_episodes, build_jev_context,
};

#[cfg(test)]
mod tests;
