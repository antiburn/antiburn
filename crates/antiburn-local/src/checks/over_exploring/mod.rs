//! Pure investigation assessment. Source adapters supply recorded evidence and
//! explicit episode boundaries. This module does not read files or call a model.

mod assessment;
mod episodes;
mod questions;

pub use questions::QUESTION_ID;

pub use assessment::{
    Abstention, Assessment, Decision, EVENT_RANGE_BYTES, MAX_EVENT_RANGES, MAX_SAMPLING_CANDIDATES,
    MAX_SUPPORTING_EVENTS, MAX_TARGETS_PER_TURN, MAX_WINDOW_TEXT_BYTES, OverExploringCheck,
    PreparedAssessment, ReadBinding, Reason, SEMANTIC_PROBABILITY_THRESHOLD, SamplingCandidate,
    SemanticOutcome, Target, Unassessed, synchronize_sampling,
};
pub use episodes::{
    EpisodeSpan, EpisodeState, InvestigationEpisode, OverExploringInput, ReadObservation,
    build_episodes, build_jev_context,
};

#[cfg(test)]
mod tests;
