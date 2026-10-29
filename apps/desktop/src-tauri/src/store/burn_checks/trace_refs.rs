use antiburn_checks::decision::DecisionAttempt;
use antiburn_checks::domain::{ActivationId, ActivationScope, FrozenCandidate};
use antiburn_checks::engine::EvaluationFailure;
use antiburn_checks::results::DecisionResult;
use sha2::{Digest, Sha256};

fn reference(kind: &[u8], parts: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"burn_check_trace_v1");
    digest.update(kind);
    for part in parts {
        digest.update(part.len().to_le_bytes());
        digest.update(part);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn activation_ref(activation: &ActivationId) -> String {
    reference(
        b"activation",
        &[&serde_json::to_vec(activation).expect("activation serializes")],
    )
}

pub(crate) fn check_ref(definition_id: &str) -> String {
    reference(b"check", &[definition_id.as_bytes()])
}

pub(crate) fn scope_ref(scope: &ActivationScope) -> String {
    reference(
        b"scope",
        &[&serde_json::to_vec(scope).expect("scope serializes")],
    )
}

pub(crate) fn report_ref(effective_id: &str) -> String {
    reference(b"report", &[effective_id.as_bytes()])
}

pub(crate) fn candidate_ref(candidate: &FrozenCandidate) -> String {
    candidate_id_ref(&candidate.fingerprint)
}

pub(crate) fn candidate_id_ref(id: &[u8; 32]) -> String {
    reference(b"candidate", &[id])
}

pub(crate) fn decision_ref(candidate: &FrozenCandidate, attempt: &DecisionAttempt) -> String {
    reference(
        b"decision",
        &[&candidate.fingerprint, attempt.decision.as_bytes()],
    )
}

pub(crate) fn provider_attempt_completed(
    candidate: &FrozenCandidate,
    attempt: &DecisionAttempt,
    result: &Result<DecisionResult, EvaluationFailure>,
    elapsed: std::time::Duration,
) {
    ::tracing::debug!(event = "burn_check_provider_attempt_completed",
        check_ref = check_ref(&candidate.activation.definition_id),
        activation_ref = activation_ref(&candidate.activation),
        candidate_ref = candidate_ref(candidate), decision_ref = decision_ref(candidate, attempt),
        generation = candidate.activation.generation.0, attempt_number = attempt.number,
        status = if result.is_ok() { "answered" } else { "failed" },
        reason = ?result.as_ref().err().map(|failure| failure.code),
        elapsed_ms = elapsed.as_millis() as u64);
}
