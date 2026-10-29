//! Transactional persistence operations required by the check runtime.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::actions::ActionPlan;
use crate::decision::{DecisionAttempt, DecisionBlock, QuestionRequirements};
use crate::definition_lifecycle::{EffectiveActivation, LifecycleTransition};
use crate::domain::{FrozenCandidate, ObservationPosition};
use crate::fit::{LocalPreparation, PreparationFailure, PreparationRecord};
use crate::results::DecisionResult;
pub use crate::results::EvaluatedAssessment;
use crate::selection::SelectionState;
use crate::source::{CommittedSourceObservation, PublicationCursor, SourceBatch};

/// Stable identity for a frozen candidate.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CandidateId(pub [u8; 32]);

impl From<&FrozenCandidate> for CandidateId {
    fn from(candidate: &FrozenCandidate) -> Self {
        Self(candidate.fingerprint)
    }
}

/// A claimed candidate and the prior branch status used to plan actions.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimedCandidate {
    /// Stable candidate ID.
    pub id: CandidateId,
    /// Immutable input selected when the candidate was created.
    pub candidate: FrozenCandidate,
    /// Prior status, if one was written in this generation.
    pub prior_status: Option<crate::compiler::StatusResult>,
    /// Monotonic Store fence. A prior process cannot settle a newer claim.
    pub fence: u64,
}

/// Durable source and selector progress for one activation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceProgress {
    /// Latest source publication committed by the Store.
    pub publication_cursor: PublicationCursor,
    /// Latest engine observation position committed by the Store.
    pub observation_position: ObservationPosition,
    /// Branch windows used to select the next candidate.
    pub selection: SelectionState,
}

impl SourceProgress {
    /// Encodes a versioned durable checkpoint. Unknown versions fail closed.
    pub fn encode(&self) -> Result<String, StoreError> {
        serde_json::to_string(&serde_json::json!({
            "version": 1,
            "publication_cursor": self.publication_cursor.0,
            "observation_position": self.observation_position.0,
            "branches": self.selection.branches,
            "seen_results": self.selection.seen_results.iter().collect::<Vec<_>>(),
        }))
        .map_err(|_| StoreError {
            message: "cannot encode selector checkpoint".into(),
        })
    }

    /// Restores exact source and observation orders without converting between them.
    pub fn decode(value: &str) -> Result<Self, StoreError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Checkpoint {
            version: u32,
            publication_cursor: u64,
            observation_position: u64,
            branches: std::collections::BTreeMap<
                crate::domain::BranchId,
                crate::selection::BranchSelection,
            >,
            seen_results: Vec<(
                (crate::domain::BranchId, String),
                crate::selection::SeenResult,
            )>,
        }
        let checkpoint: Checkpoint = serde_json::from_str(value).map_err(|_| StoreError {
            message: "invalid selector checkpoint".into(),
        })?;
        if checkpoint.version != 1 {
            return Err(StoreError {
                message: "unsupported selector checkpoint version".into(),
            });
        }
        Ok(Self {
            publication_cursor: PublicationCursor(checkpoint.publication_cursor),
            observation_position: ObservationPosition(checkpoint.observation_position),
            selection: SelectionState {
                branches: checkpoint.branches,
                seen_results: checkpoint.seen_results.into_iter().collect(),
            },
        })
    }
    /// Returns the compare-and-swap token for one source-page transaction.
    #[must_use]
    pub fn head(&self) -> SourceProgressHead {
        use sha2::{Digest, Sha256};

        SourceProgressHead {
            publication_cursor: self.publication_cursor,
            observation_position: self.observation_position,
            checkpoint_fingerprint: Sha256::digest(
                self.encode()
                    .expect("selector checkpoint serializes")
                    .as_bytes(),
            )
            .into(),
        }
    }
}

/// Durable cursor values expected before a source-page transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceProgressHead {
    /// Latest source publication committed by the Store.
    pub publication_cursor: PublicationCursor,
    /// Latest engine observation position committed by the Store.
    pub observation_position: ObservationPosition,
    /// Exact selector state, including changes caused by session deletion.
    pub checkpoint_fingerprint: [u8; 32],
}

/// The result of trying to claim a due candidate.
#[derive(Clone, Debug, PartialEq)]
pub enum CandidateClaim {
    /// The candidate is claimed and its prior status is frozen.
    Claimed(Box<ClaimedCandidate>),
    /// The Store has a durable cooldown that has not expired yet.
    NotDue { retry_at_unix_ms: u64 },
    /// The candidate is no longer valid for the active generation or evidence.
    Stale,
}

/// Exact durable model attempt rows for one candidate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionProgress {
    pub attempts: Vec<SavedDecisionAttempt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedDecisionAttempt {
    pub attempt: DecisionAttempt,
    pub outcome: DecisionAttemptOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecisionAttemptOutcome {
    DispatchIntent,
    SafeTransient,
    Unknown,
    Permanent,
    Answer(DecisionResult),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedDecisionProgress {
    pub attempts: u64,
    pub state: DecisionProgressState,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecisionProgressState {
    Next {
        attempt: DecisionAttempt,
        unfinished_decisions: u64,
    },
    Complete(EvaluatedAssessment),
    Terminal(DecisionProgressTerminal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionProgressTerminal {
    UnresolvedIntent,
    Unknown,
    Permanent,
    AttemptsExhausted,
}

/// Binds exact durable rows to authored order and required answer details.
pub fn validate_decision_progress(
    declarations: &[DecisionBlock],
    requirements: &[QuestionRequirements],
    progress: &DecisionProgress,
) -> Result<ValidatedDecisionProgress, StoreError> {
    use std::collections::BTreeMap;
    let invalid = || StoreError {
        message: "invalid saved decision progress".into(),
    };
    crate::decision::validate_requirements(declarations, requirements).map_err(|_| invalid())?;
    let mut rows = BTreeMap::new();
    for row in &progress.attempts {
        row.attempt.validate().map_err(|_| invalid())?;
        if !declarations
            .iter()
            .any(|block| block.name == row.attempt.decision)
            || rows
                .insert(
                    (row.attempt.decision.as_str(), row.attempt.number),
                    &row.outcome,
                )
                .is_some()
        {
            return Err(invalid());
        }
    }
    let attempts = u64::try_from(rows.len()).map_err(|_| invalid())?;
    let mut answers = Vec::new();
    let mut state = None;
    for (index, block) in declarations.iter().enumerate() {
        let first = rows.remove(&(block.name.as_str(), 1));
        let second = rows.remove(&(block.name.as_str(), 2));
        if state.is_some() {
            if first.is_some() || second.is_some() {
                return Err(invalid());
            }
            continue;
        }
        let (number, outcome) = match (first, second) {
            (None, None) => (1, None),
            (None, Some(_)) => return Err(invalid()),
            (Some(DecisionAttemptOutcome::SafeTransient), second) => (2, second),
            (Some(first), None) => (1, Some(first)),
            (Some(_), Some(_)) => return Err(invalid()),
        };
        match outcome {
            Some(DecisionAttemptOutcome::Answer(answer)) => {
                answer
                    .validate_for_declaration(block, requirements)
                    .map_err(|_| invalid())?;
                if answer.attempts() != number {
                    return Err(invalid());
                }
                answers.push(answer);
            }
            Some(DecisionAttemptOutcome::DispatchIntent) => {
                state = Some(DecisionProgressState::Terminal(
                    DecisionProgressTerminal::UnresolvedIntent,
                ))
            }
            Some(DecisionAttemptOutcome::Unknown) => {
                state = Some(DecisionProgressState::Terminal(
                    DecisionProgressTerminal::Unknown,
                ))
            }
            Some(DecisionAttemptOutcome::Permanent) => {
                state = Some(DecisionProgressState::Terminal(
                    DecisionProgressTerminal::Permanent,
                ))
            }
            Some(DecisionAttemptOutcome::SafeTransient) => {
                state = Some(DecisionProgressState::Terminal(
                    DecisionProgressTerminal::AttemptsExhausted,
                ))
            }
            None => {
                state = Some(DecisionProgressState::Next {
                    attempt: DecisionAttempt::new(block.name.clone(), number)
                        .map_err(|_| invalid())?,
                    unfinished_decisions: u64::try_from(declarations.len() - index)
                        .map_err(|_| invalid())?,
                })
            }
        }
    }
    if !rows.is_empty() {
        return Err(invalid());
    }
    #[derive(Serialize)]
    struct BorrowedAnswers<'a> {
        version: u8,
        decisions: &'a [&'a DecisionResult],
        attempts: u64,
    }
    crate::bounded_json::measure_json(
        &BorrowedAnswers {
            version: crate::results::RESULT_VERSION,
            decisions: &answers,
            attempts,
        },
        crate::results::RESULT_LIMITS,
    )
    .map_err(|_| invalid())?;
    let state = match state {
        Some(state) => state,
        None => {
            let evaluated = EvaluatedAssessment::new(
                declarations,
                requirements,
                answers.into_iter().cloned().collect(),
            )
            .map_err(|_| invalid())?;
            if evaluated.attempts() != attempts {
                return Err(invalid());
            }
            DecisionProgressState::Complete(evaluated)
        }
    };
    Ok(ValidatedDecisionProgress { attempts, state })
}

/// An assessment submitted for atomic acceptance.
#[derive(Clone, Debug, PartialEq)]
pub struct AssessmentAcceptance {
    /// The claimed immutable candidate.
    pub claimed: ClaimedCandidate,
    /// Validated decisions from the evaluator.
    pub evaluated: EvaluatedAssessment,
    /// Ordered actions and the resulting status, if any.
    pub actions: ActionPlan,
}

impl AssessmentAcceptance {
    /// Returns only a new status write. Retaining prior status does not write it again.
    #[must_use]
    pub fn status_write(&self) -> Option<crate::compiler::StatusResult> {
        self.actions
            .outcomes
            .iter()
            .rev()
            .find_map(|action| match action.outcome {
                crate::actions::ActionOutcomeKind::Applied { result, .. } => Some(result),
                _ => None,
            })
    }
}

/// Durable acceptance result. Deferred work retains its answer and claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptanceOutcome {
    /// History, optional status write, and watermark committed together.
    Accepted,
    /// Selected source publications need normalization before dependency validation.
    Deferred,
    /// Generation, evidence, or watermark revoked publication rights.
    Stale,
    /// This exact candidate was accepted before; no second write occurred.
    AlreadyAccepted,
}

/// A settled unsuccessful provider attempt. Only a known-safe failure permits retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptFailure {
    /// The provider proves no uncertain delivery; one retry is allowed.
    SafeTransient,
    /// Delivery or billing is uncertain. Automatic resend is prohibited.
    Unknown,
    /// A terminal transport, capability, or response-validation failure.
    Permanent,
}

/// Publication rights after a claim-bound persistence operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The operation committed for the current claim.
    Current,
    /// The claim lost publication rights. Do not send or retry this candidate.
    Stale,
}

/// Saved work returned by restart and deferred-answer recovery.
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryRecord {
    /// Effective definition needed for preparation and action planning.
    pub activation: EffectiveActivation,
    /// Exact frozen work; a received answer must never trigger another send.
    pub work: RecoveryWork,
}

/// Recoverable states after abandoned dispatches have been marked unknown.
#[derive(Clone, Debug, PartialEq)]
pub enum RecoveryWork {
    /// No unresolved dispatch exists; the candidate can be claimed.
    Pending {
        candidate: FrozenCandidate,
        resumed: bool,
    },
    /// Retry acceptance using the persisted answer and original claim.
    Answered {
        claimed: ClaimedCandidate,
        evaluated: EvaluatedAssessment,
    },
}

/// A persistence operation failed without claiming the work was completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError {
    /// Safe detail for the caller; storage implementations must not include payloads.
    pub message: String,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StoreError {}

/// Atomic storage boundary owned by the desktop Store implementation.
///
/// Each method is one transaction boundary. Implementations must make source-page
/// application, publication acknowledgment, selector progress, and candidates
/// atomic, and must revalidate generation, evidence dependencies, and watermarks
/// during acceptance.
pub trait CheckStore: Send + Sync {
    /// Atomically persists a lifecycle transition and bootstrap boundary.
    fn activate(
        &self,
        definition_id: &str,
        transition: &LifecycleTransition,
        activation: Option<&EffectiveActivation>,
    ) -> Result<(), StoreError>;

    /// Loads durable publication, observation, and selector progress after restart.
    fn load_source_progress(
        &self,
        activation: &EffectiveActivation,
    ) -> Result<SourceProgress, StoreError>;

    /// Atomically applies a publication page, acknowledges it, and stores selector progress.
    ///
    /// `expected` must match the durable progress head. `next` must include the
    /// page's final publication cursor, the committed observation position, and
    /// the selector state produced from this page. A failure must leave all of
    /// those values and the candidates unchanged.
    /// The engine changes stale and duplicate events to `SourceEvent::Ignored`.
    /// Each `window_invalidated` observation revokes earlier branch membership
    /// dependencies, including candidates that do not select that identity.
    fn apply_source_page(
        &self,
        activation: &EffectiveActivation,
        expected: SourceProgressHead,
        page: &SourceBatch,
        observations: &[CommittedSourceObservation],
        next: &SourceProgress,
        candidates: &[FrozenCandidate],
    ) -> Result<(), StoreError>;

    /// Records a candidate that could not enter the bounded runtime queue.
    fn record_overflow(&self, candidate: &FrozenCandidate) -> Result<(), StoreError>;

    /// Reads a keyset page, including answered work. `after` is exclusive.
    /// Call until an empty page; a full first page is never the complete recovery set.
    fn load_recovery(
        &self,
        after: Option<&CandidateId>,
        limit: usize,
    ) -> Result<Vec<RecoveryRecord>, StoreError>;

    /// Returns requested IDs whose durable state is proven terminal.
    ///
    /// Use a closed terminal-state list. Omit unfinished, missing, unknown, and
    /// out-of-scope IDs. Each call contains at most the pending queue capacity.
    /// Propagate read failures; missing state is not permission to remove work.
    fn load_terminal_candidate_ids(
        &self,
        candidate_ids: &[CandidateId],
    ) -> Result<Vec<CandidateId>, StoreError>;

    /// Claims one due candidate while enforcing generation and branch single flight.
    fn claim_due(
        &self,
        candidate: &FrozenCandidate,
        now_unix_ms: u64,
        bypass_check_cooldown: bool,
    ) -> Result<CandidateClaim, StoreError>;

    /// Releases only the exact claim before its next dispatch intent exists.
    /// Current permits a fresh claim; stale forbids restoring the old candidate.
    fn release_pre_dispatch_claim(
        &self,
        claimed: &ClaimedCandidate,
    ) -> Result<ClaimOutcome, StoreError>;

    /// Loads exact attempts under the current claim, including settled siblings.
    fn load_decision_progress(
        &self,
        claimed: &ClaimedCandidate,
    ) -> Result<DecisionProgress, StoreError>;

    /// Loads the immutable preparation saved before an earlier attempt.
    fn load_preparation(
        &self,
        _claimed: &ClaimedCandidate,
    ) -> Result<Option<PreparationRecord>, StoreError> {
        Ok(None)
    }

    /// Saves provider preparation before any dispatch intent exists.
    fn record_preparation(
        &self,
        _claimed: &ClaimedCandidate,
        _preparation: &PreparationRecord,
    ) -> Result<ClaimOutcome, StoreError> {
        Ok(ClaimOutcome::Current)
    }

    /// Saves local preparation, its empty answer, and first cooldown atomically.
    /// Repeating the same answered claim must not change its start or cooldown.
    fn record_local_start(
        &self,
        claimed: &ClaimedCandidate,
        now_unix_ms: u64,
        cooldown: Duration,
        preparation: &LocalPreparation,
    ) -> Result<ClaimOutcome, StoreError>;

    /// Saves a typed local rejection and settles without a new provider attempt.
    fn record_preparation_rejection(
        &self,
        claimed: &ClaimedCandidate,
        failure: &PreparationFailure,
    ) -> Result<ClaimOutcome, StoreError> {
        self.record_not_assessed(claimed, &failure.detail)
    }

    /// Persists dispatch intent before the evaluator call begins. Stale forbids a send.
    fn record_dispatch_intent(
        &self,
        claimed: &ClaimedCandidate,
        attempted_at_unix_ms: u64,
        cooldown: Duration,
        attempt: &DecisionAttempt,
    ) -> Result<ClaimOutcome, StoreError>;

    /// Atomically settles one exact attempt and saves its typed diagnostic.
    /// This also applies after source revocation.
    /// Stale confirms settlement but forbids a retry.
    fn record_attempt_failure(
        &self,
        claimed: &ClaimedCandidate,
        attempt: &DecisionAttempt,
        outcome: AttemptFailure,
        diagnostic: &crate::engine::EvaluationDiagnostic,
    ) -> Result<ClaimOutcome, StoreError>;

    /// Persists a received validated response before acceptance is attempted.
    fn record_decision_answer(
        &self,
        claimed: &ClaimedCandidate,
        attempt: &DecisionAttempt,
        evaluated: &DecisionResult,
    ) -> Result<ClaimOutcome, StoreError>;

    /// Atomically accepts history, action outcomes, status, and watermark.
    fn accept(&self, assessment: &AssessmentAcceptance) -> Result<AcceptanceOutcome, StoreError>;

    /// Persists a terminal execution failure without writing status or watermark.
    fn record_failure(
        &self,
        claimed: &ClaimedCandidate,
        detail: &str,
    ) -> Result<ClaimOutcome, StoreError>;

    /// Persists a typed failure with its terminal assessment.
    fn record_evaluation_failure(
        &self,
        claimed: &ClaimedCandidate,
        diagnostic: &crate::engine::EvaluationDiagnostic,
    ) -> Result<ClaimOutcome, StoreError> {
        self.record_failure(claimed, &diagnostic.detail)
    }

    /// Persists a known not-assessed outcome without creating provider dispatch intent.
    fn record_not_assessed(
        &self,
        claimed: &ClaimedCandidate,
        detail: &str,
    ) -> Result<ClaimOutcome, StoreError>;
}
