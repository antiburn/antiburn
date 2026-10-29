use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum StoreOperation {
    StartupRecovery,
    DefinitionInventory,
    PauseInventory,
    LifecycleRead,
    LifecycleWrite,
    ActivationRegistration,
    PublicationHead,
    TerminalInventory,
    SourceProgress,
    SourcePage,
    Overflow,
    Recovery,
    Claim,
    DispatchIntent,
    DecisionProgress,
    LocalStart,
    ClaimRelease,
    PreparationRead,
    PreparationWrite,
    PreparationRejection,
    EvaluationFailure,
    AttemptFailure,
    DecisionAnswer,
    Acceptance,
    FailureSettlement,
    NotAssessed,
}

impl StoreOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::StartupRecovery => "startup_recovery",
            Self::DefinitionInventory => "definition_inventory",
            Self::PauseInventory => "pause_inventory",
            Self::LifecycleRead => "lifecycle_read",
            Self::LifecycleWrite => "lifecycle_write",
            Self::ActivationRegistration => "activation_registration",
            Self::PublicationHead => "publication_head",
            Self::TerminalInventory => "terminal_inventory",
            Self::SourceProgress => "source_progress",
            Self::SourcePage => "source_page",
            Self::Overflow => "overflow",
            Self::Recovery => "recovery",
            Self::Claim => "claim",
            Self::DispatchIntent => "dispatch_intent",
            Self::DecisionProgress => "decision_progress",
            Self::LocalStart => "local_start",
            Self::ClaimRelease => "claim_release",
            Self::PreparationRead => "preparation_read",
            Self::PreparationWrite => "preparation_write",
            Self::PreparationRejection => "preparation_rejection",
            Self::EvaluationFailure => "evaluation_failure",
            Self::AttemptFailure => "attempt_failure",
            Self::DecisionAnswer => "decision_answer",
            Self::Acceptance => "acceptance",
            Self::FailureSettlement => "failure_settlement",
            Self::NotAssessed => "not_assessed",
        }
    }
}

pub(super) enum StoreTraceContext<'a> {
    Scope,
    Definition(&'a str),
    Activation(&'a ActivationId),
    Candidate(&'a FrozenCandidate),
    Claim(&'a ClaimedCandidate, Option<&'a DecisionAttempt>),
}

impl StoreCheckStore {
    pub(super) fn claim_store_result<T>(
        &self,
        claimed: &ClaimedCandidate,
        operation: StoreOperation,
        attempt: Option<&DecisionAttempt>,
        result: Result<T>,
    ) -> Result<T, StoreError> {
        self.store_result(
            StoreTraceContext::Claim(claimed, attempt),
            operation,
            result,
        )
    }

    pub(super) fn store_result<T>(
        &self,
        context: StoreTraceContext<'_>,
        operation: StoreOperation,
        result: Result<T>,
    ) -> Result<T, StoreError> {
        let (definition, activation, candidate, fence, attempt) = match context {
            StoreTraceContext::Scope => (None, None, None, None, None),
            StoreTraceContext::Definition(id) => (Some(id), None, None, None, None),
            StoreTraceContext::Activation(id) => {
                (Some(id.definition_id.as_str()), Some(id), None, None, None)
            }
            StoreTraceContext::Candidate(candidate) => (
                Some(candidate.activation.definition_id.as_str()),
                Some(&candidate.activation),
                Some(candidate),
                None,
                None,
            ),
            StoreTraceContext::Claim(claimed, attempt) => (
                Some(claimed.candidate.activation.definition_id.as_str()),
                Some(&claimed.candidate.activation),
                Some(&claimed.candidate),
                Some(claimed.fence),
                attempt,
            ),
        };
        let scope = super::super::trace_refs::scope_ref(&self.scope);
        let key = candidate
            .map(candidate_ref)
            .or_else(|| activation.map(activation_ref))
            .or_else(|| definition.map(check_ref))
            .unwrap_or_else(|| scope.clone());
        let failures = if operation == StoreOperation::StartupRecovery {
            static STARTUP: std::sync::OnceLock<FailureLog> = std::sync::OnceLock::new();
            STARTUP.get_or_init(Default::default)
        } else {
            &self.store_failure_traces
        };
        match result {
            Ok(value) => {
                failures.changed(key, operation.as_str(), None);
                Ok(value)
            }
            Err(error) => {
                let cause = FailureCause::classify(&error);
                let changed = failures.changed(key, operation.as_str(), Some(cause));
                if changed {
                    ::tracing::error!(event = "burn_check_store_operation_failed", stage = operation.as_str(),
                        reason = cause.category, sqlite_code = cause.sqlite_code, io_kind = ?cause.io_kind,
                        scope_ref = scope, check_ref = definition.map(check_ref),
                        activation_ref = activation.map(activation_ref),
                        candidate_ref = candidate.map(candidate_ref),
                        decision_ref = candidate.zip(attempt).map(|(candidate, attempt)| decision_ref(candidate, attempt)),
                        claim_fence = fence);
                }
                Err(store_error(error))
            }
        }
    }

    pub(super) fn trace_recovery_page(
        &self,
        after: Option<[u8; 32]>,
        examined: usize,
        skipped: usize,
        records: &[RecoveryRecord],
        elapsed: Duration,
    ) {
        if records.is_empty() {
            return;
        }
        let answered = records
            .iter()
            .filter(|record| matches!(record.work, RecoveryWork::Answered { .. }))
            .count();
        let mut digest = Sha256::new();
        digest.update(examined.to_le_bytes());
        digest.update(skipped.to_le_bytes());
        for record in records {
            match &record.work {
                RecoveryWork::Pending { candidate, resumed } => {
                    digest.update(candidate.fingerprint);
                    digest.update([0, u8::from(*resumed)]);
                }
                RecoveryWork::Answered { claimed, .. } => {
                    digest.update(claimed.id.0);
                    digest.update([1]);
                    digest.update(claimed.fence.to_le_bytes());
                }
            }
        }
        let signature = digest.finalize().into();
        let mut previous = self
            .recovery_traces
            .lock()
            .expect("trace mutex is not poisoned");
        if previous.get(&after) == Some(&signature) {
            return;
        }
        if previous.len() >= antiburn_checks::engine::MAX_PENDING_CANDIDATES
            && !previous.contains_key(&after)
        {
            return;
        }
        previous.insert(after, signature);
        ::tracing::debug!(
            event = "burn_check_recovery_page",
            examined,
            skipped,
            recovered = records.len(),
            answered,
            elapsed_ms = elapsed.as_millis() as u64
        );
    }

    pub(super) fn trace_acceptance(
        &self,
        claimed: &ClaimedCandidate,
        status: &'static str,
        reason: &'static str,
        elapsed: Duration,
    ) {
        let mut deferred = self
            .deferred_traces
            .lock()
            .expect("trace mutex is not poisoned");
        if status == "deferred" {
            if deferred.get(&claimed.id.0) == Some(&reason) {
                return;
            }
            if deferred.len() >= antiburn_checks::engine::MAX_PENDING_CANDIDATES
                && !deferred.contains_key(&claimed.id.0)
            {
                return;
            }
            deferred.insert(claimed.id.0, reason);
        } else {
            deferred.remove(&claimed.id.0);
        }
        ::tracing::debug!(
            event = "burn_check_acceptance",
            activation_ref = activation_ref(&claimed.candidate.activation),
            candidate_ref = candidate_ref(&claimed.candidate),
            generation = claimed.candidate.activation.generation.0,
            claim_fence = claimed.fence,
            status,
            reason,
            elapsed_ms = elapsed.as_millis() as u64,
        );
    }
}
