//! Pure investigation assessment. Source adapters supply recorded evidence and
//! explicit episode boundaries. This module does not read files or call a model.

mod assessment;
mod episodes;
mod explanation;
mod questions;

pub use questions::QUESTION_ID;

pub use assessment::{
    Abstention, Assessment, Decision, ExplanationBasis, MAX_CANDIDATE_STATE_BYTES,
    MAX_ESTIMATED_INPUT_TOKENS, MAX_PACKED_REQUEST_BYTES, MAX_SAMPLING_CANDIDATES,
    MAX_TARGETS_PER_TURN, OverExploringCheck, PreparedAssessment, ReadBinding, ReadRelationship,
    ReadSourceSnippet, Reason, SEMANTIC_PROBABILITY_THRESHOLD, SamplingCandidate, SemanticOutcome,
    Target, Unassessed, synchronize_sampling,
};
pub use episodes::{
    EpisodeSpan, EpisodeState, InvestigationEpisode, OverExploringInput, ReadObservation,
    build_episodes, build_jev_context, has_known_returned_extent,
};

pub fn is_absolute_source_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

#[cfg(test)]
mod tests;
