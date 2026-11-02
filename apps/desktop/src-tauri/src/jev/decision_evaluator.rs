//! Selected System One adapter for complete named decision blocks.

use antiburn_checks::check_store::AttemptFailure;
use antiburn_checks::decision::{
    DecisionAttempt, DecisionBlock, DecisionInput, Question, QuestionRequirements, attempt_id,
};
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::domain::FrozenCandidate;
use antiburn_checks::engine::{
    DecisionEvaluator, DecisionFuture, EvaluationFailure, EvaluationFailureCode,
    PreparationValidationPurpose,
};
use antiburn_checks::expression_eval::EvaluationBudget;
use antiburn_checks::fit::{PreparationFailure, PreparationFailureCode};
use antiburn_checks::prepared_assessment::PreparedAssessment;
use antiburn_checks::results::{
    ChoiceResult, DecisionResult, NamedAnswer, NoulResult, QuestionResult, ScoreResult,
};
use antiburn_local::analysis::jev::{
    JevAnswer, JevError, JevRequest, JevResponse, JevResponseObservation, JevUsage,
    MAX_RESPONSE_BYTES, capabilities::ModelCapabilities, validate_jev_request_with_capabilities,
};
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(test)]
use std::time::{Duration, Instant};

mod preparation;
use preparation::{
    SanitizedConnectionDescriptor, prepare_selected_assessment, validate_selected_assessment,
};
pub(crate) use preparation::{
    prepared_profile_provenance, validate_prepared_assessment, validate_prepared_for_connection,
};

const MAX_RESIDENT_REQUEST_BYTES: usize = 512 * 1024;
const NATIVE_MAX_RESIDENT_REQUEST_BYTES: usize = 832 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DecisionProvenance {
    pub(crate) provider: super::config::SystemOneProvider,
    pub(crate) model: String,
    pub(crate) model_revision: Option<String>,
    pub(crate) connection_revision: u64,
}

/// Records one candidate and named attempt before answer validation.
pub(crate) type DecisionUsageRecorder = Arc<
    dyn Fn(&str, &DecisionAttempt, Option<JevResponseObservation>) -> Result<(), String>
        + Send
        + Sync,
>;

pub(crate) struct TypeSafeDecisionEvaluator {
    transport: super::dispatch::DecisionTransport,
    descriptor: SanitizedConnectionDescriptor,
    capabilities: ModelCapabilities,
    is_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    usage: Option<DecisionUsageRecorder>,
    authentication_rejected: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    #[cfg(test)]
    diagnostics: Option<Arc<dyn Fn(ProviderAttemptDiagnostic) + Send + Sync>>,
}

impl TypeSafeDecisionEvaluator {
    #[cfg(test)]
    pub(crate) fn new(client: super::client::TypeSafeClient) -> Self {
        let capabilities = ModelCapabilities::jev_native();
        Self {
            transport: super::dispatch::DecisionTransport::Jev {
                client,
                endpoint: super::config::system_one_endpoint(),
            },
            descriptor: SanitizedConnectionDescriptor::from_connection(
                &super::config::SystemOneConnection::jev_default(),
                &capabilities,
            )
            .expect("valid test connection"),
            capabilities,
            is_cancelled: Arc::new(|| false),
            usage: None,
            authentication_rejected: None,
            diagnostics: None,
        }
    }

    pub(crate) fn from_selected_snapshot(
        connection: super::config::SystemOneConnection,
        credential: Option<String>,
        capabilities: ModelCapabilities,
    ) -> Result<Self, JevError> {
        if connection.model != capabilities.model
            || connection
                .model_revision
                .as_ref()
                .is_some_and(|revision| capabilities.model_revision.as_ref() != Some(revision))
        {
            return Err(JevError::UnsupportedModel);
        }
        let descriptor =
            SanitizedConnectionDescriptor::from_connection(&connection, &capabilities)?;
        preparation::admit_connection(&descriptor, &capabilities)?;
        let transport =
            super::dispatch::DecisionTransport::from_connection(&connection, credential)?;
        Ok(Self {
            transport,
            descriptor,
            capabilities,
            is_cancelled: Arc::new(|| false),
            usage: None,
            authentication_rejected: None,
            #[cfg(test)]
            diagnostics: None,
        })
    }

    pub(crate) fn with_cancellation(
        mut self,
        is_cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.is_cancelled = Arc::new(is_cancelled);
        self
    }

    pub(crate) fn with_usage(mut self, recorder: DecisionUsageRecorder) -> Self {
        self.usage = Some(recorder);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_native_test_endpoint(mut self, endpoint: String) -> Self {
        let super::dispatch::DecisionTransport::Jev {
            endpoint: selected, ..
        } = &mut self.transport
        else {
            panic!("A native loopback endpoint requires the native transport");
        };
        *selected = endpoint;
        self
    }

    pub(crate) fn with_authentication_rejection(
        mut self,
        reject: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.authentication_rejected = Some(Arc::new(reject));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_diagnostics(
        mut self,
        diagnostics: Arc<dyn Fn(ProviderAttemptDiagnostic) + Send + Sync>,
    ) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }
}

#[cfg(test)]
pub(crate) struct ProviderAttemptDiagnostic {
    pub(crate) request_id: String,
    pub(crate) decision: String,
    pub(crate) attempt_number: u8,
    pub(crate) outcome: AttemptOutcome,
    pub(crate) latency: Duration,
}

impl DecisionEvaluator for TypeSafeDecisionEvaluator {
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        evaluation_budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        prepare_selected_assessment(
            activation,
            inputs,
            evaluation_budget,
            &self.capabilities,
            &self.descriptor,
        )
    }

    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        match purpose {
            PreparationValidationPurpose::Publication => {
                validate_prepared_assessment(activation, prepared)
            }
            PreparationValidationPurpose::Dispatch => {
                if (self.is_cancelled)() {
                    return Err(unavailable_preparation());
                }
                validate_selected_assessment(
                    activation,
                    prepared,
                    &self.capabilities,
                    &self.descriptor,
                )
            }
        }
    }

    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            if (self.is_cancelled)() {
                return Err(EvaluationFailure {
                    code: EvaluationFailureCode::Cancelled,
                    kind: AttemptFailure::Permanent,
                    detail: "The decision was cancelled before dispatch.".into(),
                    attempts: 0,
                });
            }
            attempt.validate().map_err(|_| {
                invalid_preparation(preparation::invalid(
                    "The named decision attempt is invalid.",
                ))
            })?;
            self.validate_prepared(activation, prepared, PreparationValidationPurpose::Dispatch)
                .map_err(invalid_preparation)?;
            let declaration = activation
                .compiled
                .decisions
                .iter()
                .find(|block| block.name == attempt.decision)
                .ok_or_else(|| {
                    invalid_preparation(preparation::invalid(
                        "The attempt names an undeclared decision.",
                    ))
                })?;
            let block = prepared
                .decisions
                .iter()
                .find(|block| block.decision == attempt.decision)
                .ok_or_else(|| {
                    invalid_preparation(preparation::invalid("The named request is missing."))
                })?;
            let request: JevRequest =
                serde_json::from_value(block.request.clone()).map_err(|_| {
                    invalid_preparation(preparation::invalid("The saved request is invalid."))
                })?;
            let candidate_id = candidate
                .fingerprint
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let request_id = attempt_id(&candidate.fingerprint, attempt);
            #[cfg(test)]
            let started = Instant::now();
            let outcome = evaluate_attempt(
                &self.transport,
                AttemptRequest {
                    declaration,
                    requirements: &activation.compiled.question_requirements(),
                    request: &request,
                    capabilities: &self.capabilities,
                    attempt,
                    request_id: &request_id,
                },
                || (self.is_cancelled)(),
                |usage| {
                    self.usage.as_ref().map_or(Ok(()), |record| {
                        record(&candidate_id, attempt, usage)
                            .map_err(|_| JevError::ProgressStorageFailure)
                    })
                },
            )
            .await;
            #[cfg(test)]
            if let Some(diagnostics) = &self.diagnostics {
                diagnostics(ProviderAttemptDiagnostic {
                    request_id,
                    decision: attempt.decision.clone(),
                    attempt_number: attempt.number,
                    outcome: outcome.clone(),
                    latency: started.elapsed(),
                });
            }
            settle_authentication_rejection(
                &outcome,
                self.authentication_rejected.as_ref(),
                attempt.number,
            )?;
            evaluation_result(
                outcome,
                attempt.number,
                self.descriptor.provider,
                &self.descriptor.model,
            )
        })
    }
}

fn unavailable_preparation() -> PreparationFailure {
    PreparationFailure {
        code: PreparationFailureCode::ProviderUnavailable,
        detail: "The selected connection is no longer current.".into(),
        report: None,
        context: Default::default(),
    }
}

fn invalid_preparation(error: PreparationFailure) -> EvaluationFailure {
    EvaluationFailure {
        code: EvaluationFailureCode::InvalidRequest,
        kind: AttemptFailure::Permanent,
        detail: error.detail,
        attempts: 0,
    }
}

fn settle_authentication_rejection(
    outcome: &AttemptOutcome,
    reject: Option<&Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    attempts: u8,
) -> Result<(), EvaluationFailure> {
    if matches!(
        outcome,
        AttemptOutcome::Failed {
            error: JevError::AuthenticationRejected,
            ..
        }
    ) && let Some(reject) = reject
        && let Err(error) = reject()
    {
        return Err(EvaluationFailure {
            code: EvaluationFailureCode::StorageFailure,
            kind: AttemptFailure::Permanent,
            detail: format!("Could not record provider authentication rejection: {error}"),
            attempts,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SuccessfulAttempt {
    pub(crate) result: DecisionResult,
    pub(crate) usage: JevUsage,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AttemptOutcome {
    Answer(SuccessfulAttempt),
    KnownTransientFailure(JevError),
    Unknown,
    Failed {
        error: JevError,
        usage: Option<JevUsage>,
        rejected_answers: Option<BTreeMap<String, JevAnswer>>,
    },
}

fn evaluation_result(
    outcome: AttemptOutcome,
    attempt_number: u8,
    provider: super::config::SystemOneProvider,
    model: &str,
) -> Result<DecisionResult, EvaluationFailure> {
    let (code, kind, detail) = match outcome {
        AttemptOutcome::Answer(answer) => return Ok(answer.result),
        AttemptOutcome::Unknown => (
            EvaluationFailureCode::DeliveryUnknown,
            AttemptFailure::Unknown,
            format!("{provider:?} model {model}: request outcome is unknown."),
        ),
        AttemptOutcome::KnownTransientFailure(error) => (
            diagnostic_code(&error),
            AttemptFailure::SafeTransient,
            format!("{provider:?} model {model}: transient failure: {error:?}"),
        ),
        AttemptOutcome::Failed { error, .. } => (
            diagnostic_code(&error),
            AttemptFailure::Permanent,
            format!("{provider:?} model {model}: evaluation failed: {error:?}"),
        ),
    };
    Err(EvaluationFailure {
        code,
        kind,
        detail,
        attempts: attempt_number,
    })
}

fn diagnostic_code(error: &JevError) -> EvaluationFailureCode {
    match error {
        JevError::AuthenticationRejected => EvaluationFailureCode::AuthenticationRejected,
        JevError::RateLimited { .. } => EvaluationFailureCode::RateLimited,
        JevError::ProviderOverloaded { .. } => EvaluationFailureCode::ProviderOverloaded,
        JevError::ProviderUnavailable => EvaluationFailureCode::ProviderUnavailable,
        JevError::RequestOutcomeUnknown => EvaluationFailureCode::DeliveryUnknown,
        JevError::ProgressStorageFailure => EvaluationFailureCode::StorageFailure,
        JevError::Cancelled => EvaluationFailureCode::Cancelled,
        JevError::ContextRejected => EvaluationFailureCode::ContextRejected,
        JevError::ResponseModelMismatch
        | JevError::ResponseAnswerCountMismatch
        | JevError::ResponseAnswerMissing
        | JevError::ResponseAnswerTypeMismatch
        | JevError::InvalidChoiceDistribution
        | JevError::InvalidNoulProbability
        | JevError::InvalidScoreDistribution
        | JevError::InvalidProbabilitySum
        | JevError::ResponseTooLarge
        | JevError::ResponseDecode
        | JevError::ResponseUsageExceeded => EvaluationFailureCode::InvalidResponse,
        JevError::RequestSerialization
        | JevError::EmptyQuestions
        | JevError::QuestionLimitExceeded
        | JevError::UnsupportedModel
        | JevError::RequestTooLarge { .. }
        | JevError::RequestTokenLimitExceeded { .. }
        | JevError::WorkItemHasNoAnswers
        | JevError::InvalidRequestSchema
        | JevError::InvalidCheckContext
        | JevError::InvalidCheckPlan => EvaluationFailureCode::InvalidRequest,
    }
}

pub(crate) fn validate_response(
    declaration: &DecisionBlock,
    requirements: &[QuestionRequirements],
    request: &JevRequest,
    response: JevResponse,
    capabilities: &ModelCapabilities,
    attempts: u8,
    provider: super::config::SystemOneProvider,
) -> Result<DecisionResult, JevError> {
    if provider == super::config::SystemOneProvider::Jev {
        antiburn_local::analysis::jev::validate_native_jev_response_with_capabilities(
            &response,
            request,
            capabilities,
        )?;
    } else {
        antiburn_local::analysis::jev::validate_jev_response_with_capabilities(
            &response,
            request,
            capabilities,
        )?;
    }
    let mut raw = response.answers;
    let mut answers = Vec::with_capacity(declaration.outputs.len());
    for output in &declaration.outputs {
        let answer = raw
            .remove(&output.name)
            .ok_or(JevError::ResponseAnswerMissing)?;
        let result = match (&output.question, answer) {
            (
                question @ Question::Choice { .. },
                JevAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) => {
                let support = capabilities
                    .output
                    .choice
                    .ok_or(JevError::UnsupportedModel)?;
                QuestionResult::Choice(
                    ChoiceResult::new(
                        question,
                        choice,
                        support.probabilities.then_some(probabilities),
                        support.confidence.then_some(confidence),
                    )
                    .map_err(|_| JevError::InvalidChoiceDistribution)?,
                )
            }
            (
                question @ Question::Score { criteria, .. },
                JevAnswer::Score {
                    score,
                    probabilities,
                    confidence,
                    ..
                },
            ) => {
                let support = capabilities
                    .output
                    .score
                    .ok_or(JevError::UnsupportedModel)?;
                let probabilities = if support.probabilities {
                    Some(
                        (0..criteria.len())
                            .map(|index| {
                                probabilities
                                    .get(&index.to_string())
                                    .copied()
                                    .ok_or(JevError::InvalidScoreDistribution)
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                } else {
                    None
                };
                QuestionResult::Score(
                    ScoreResult::new(
                        question,
                        score,
                        probabilities,
                        support.confidence.then_some(confidence),
                    )
                    .map_err(|_| JevError::InvalidScoreDistribution)?,
                )
            }
            (Question::Noul { .. }, JevAnswer::Noul { noul }) if capabilities.output.noul => {
                QuestionResult::Noul(
                    NoulResult::new(noul).map_err(|_| JevError::InvalidNoulProbability)?,
                )
            }
            _ => return Err(JevError::ResponseAnswerTypeMismatch),
        };
        answers
            .push(NamedAnswer::new(&output.name, result).map_err(|_| JevError::InvalidCheckPlan)?);
    }
    DecisionResult::new(declaration, requirements, answers, attempts)
        .map_err(|_| JevError::InvalidCheckPlan)
}

pub(crate) fn classify_outcome(
    declaration: &DecisionBlock,
    requirements: &[QuestionRequirements],
    request: &JevRequest,
    result: Result<JevResponse, JevError>,
    capabilities: &ModelCapabilities,
    attempts: u8,
    provider: super::config::SystemOneProvider,
) -> AttemptOutcome {
    match result {
        Ok(response) => {
            let usage = response.usage;
            #[cfg(test)]
            let rejected_answers = Some(response.answers.clone());
            #[cfg(not(test))]
            let rejected_answers = None;
            match validate_response(
                declaration,
                requirements,
                request,
                response,
                capabilities,
                attempts,
                provider,
            ) {
                Ok(result) => AttemptOutcome::Answer(SuccessfulAttempt { result, usage }),
                Err(error) => AttemptOutcome::Failed {
                    error,
                    usage: Some(usage),
                    rejected_answers,
                },
            }
        }
        Err(JevError::RequestOutcomeUnknown) => AttemptOutcome::Unknown,
        Err(error @ (JevError::RateLimited { .. } | JevError::ProviderOverloaded { .. })) => {
            AttemptOutcome::KnownTransientFailure(error)
        }
        Err(error) => AttemptOutcome::Failed {
            error,
            usage: None,
            rejected_answers: None,
        },
    }
}

struct AttemptRequest<'a> {
    declaration: &'a DecisionBlock,
    requirements: &'a [QuestionRequirements],
    request: &'a JevRequest,
    capabilities: &'a ModelCapabilities,
    attempt: &'a DecisionAttempt,
    request_id: &'a str,
}

async fn evaluate_attempt(
    transport: &super::dispatch::DecisionTransport,
    prepared: AttemptRequest<'_>,
    is_cancelled: impl Fn() -> bool + Sync,
    record_usage: impl Fn(Option<JevResponseObservation>) -> Result<(), JevError> + Sync,
) -> AttemptOutcome {
    let AttemptRequest {
        declaration,
        requirements,
        request,
        capabilities,
        attempt,
        request_id,
    } = prepared;
    let request_resident_bytes = match resident_bytes(request, capabilities, transport.provider()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return AttemptOutcome::Failed {
                error,
                usage: None,
                rejected_answers: None,
            };
        }
    };
    let known_usage = std::sync::Mutex::new(None);
    let result = super::dispatch::dispatch_decision_attempt(
        transport,
        request,
        capabilities,
        request_resident_bytes,
        request_id,
        is_cancelled,
        |usage| {
            if let Some(observation) = &usage {
                *known_usage.lock().expect("usage mutex is not poisoned") = Some(observation.usage);
            }
            record_usage(usage)
        },
    )
    .await;
    let mut outcome = classify_outcome(
        declaration,
        requirements,
        request,
        result,
        capabilities,
        attempt.number,
        transport.provider(),
    );
    if let AttemptOutcome::Failed { usage, .. } = &mut outcome {
        *usage = usage.or(*known_usage.lock().expect("usage mutex is not poisoned"));
    }
    outcome
}

pub(crate) fn resident_bytes(
    request: &JevRequest,
    capabilities: &ModelCapabilities,
    provider: super::config::SystemOneProvider,
) -> Result<usize, JevError> {
    let request_bytes = validate_jev_request_with_capabilities(request, capabilities)?;
    let response_bytes = capabilities
        .response_body_bytes
        .value
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_RESPONSE_BYTES);
    let bytes = request_bytes
        .saturating_mul(2)
        .saturating_add(response_bytes);
    let maximum = if provider == super::config::SystemOneProvider::Jev {
        NATIVE_MAX_RESIDENT_REQUEST_BYTES
    } else {
        MAX_RESIDENT_REQUEST_BYTES
    };
    if bytes > maximum {
        return Err(JevError::RequestTooLarge { bytes, maximum });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
