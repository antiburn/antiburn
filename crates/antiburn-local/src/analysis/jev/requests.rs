//! Request contracts, answer validation, and provider prompt rendering.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    capabilities, json_count, limit_as_usize, valid_probability, validate_probability_sum,
};

/// Sanitized failures shared by request preparation, TypeSafe transport, and
/// answer validation. Variants never contain credentials, request bodies, or
/// provider response text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevError {
    RequestSerialization,
    EmptyQuestions,
    QuestionLimitExceeded,
    UnsupportedModel,
    RequestTooLarge { bytes: usize, maximum: usize },
    RequestTokenLimitExceeded { tokens: u64, maximum: u64 },
    ResponseModelMismatch,
    ResponseAnswerCountMismatch,
    ResponseAnswerMissing,
    ResponseAnswerTypeMismatch,
    InvalidChoiceDistribution,
    InvalidNoulProbability,
    InvalidScoreDistribution,
    InvalidProbabilitySum,
    WorkItemHasNoAnswers,
    AuthenticationRejected,
    InvalidRequestSchema,
    ContextRejected,
    RateLimited { retry_after: Option<Duration> },
    ProviderOverloaded { retry_after: Option<Duration> },
    ProviderUnavailable,
    RequestOutcomeUnknown,
    ResponseTooLarge,
    ResponseDecode,
    ResponseUsageExceeded,
    ProgressStorageFailure,
    Cancelled,
    InvalidCheckContext,
    InvalidCheckPlan,
}

impl fmt::Display for JevError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestSerialization => {
                formatter.write_str("Jev request could not be serialized")
            }
            Self::EmptyQuestions => formatter.write_str("Jev request has no questions"),
            Self::ContextRejected => {
                formatter.write_str("The provider rejected the request context")
            }
            Self::QuestionLimitExceeded => {
                formatter.write_str("Jev request exceeds the question limit")
            }
            Self::UnsupportedModel => formatter.write_str("Jev request uses an unsupported model"),
            Self::RequestTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "Jev request is {bytes} bytes; the limit is {maximum} bytes"
                )
            }
            Self::RequestTokenLimitExceeded { tokens, maximum } => {
                write!(
                    formatter,
                    "Jev request estimate is {tokens} tokens; the limit is {maximum} tokens"
                )
            }
            Self::ResponseModelMismatch => {
                formatter.write_str("Jev response model does not match the request")
            }
            Self::ResponseAnswerCountMismatch => {
                formatter.write_str("Jev response has an unexpected answer count")
            }
            Self::ResponseAnswerMissing => formatter.write_str("Jev response is missing an answer"),
            Self::ResponseAnswerTypeMismatch => {
                formatter.write_str("Jev response answer has the wrong type")
            }
            Self::InvalidChoiceDistribution => {
                formatter.write_str("Jev response has an invalid choice distribution")
            }
            Self::InvalidNoulProbability => {
                formatter.write_str("Jev response has an invalid Noul probability")
            }
            Self::InvalidScoreDistribution => {
                formatter.write_str("Jev response has an invalid score distribution")
            }
            Self::InvalidProbabilitySum => {
                formatter.write_str("Jev response probabilities do not sum to one")
            }
            Self::WorkItemHasNoAnswers => {
                formatter.write_str("Jev response has no answers for a work item")
            }
            Self::AuthenticationRejected => formatter.write_str("TypeSafe rejected the API key"),
            Self::InvalidRequestSchema => {
                formatter.write_str("TypeSafe rejected the request schema")
            }
            Self::RateLimited { .. } => formatter.write_str("TypeSafe rate-limited the request"),
            Self::ProviderOverloaded { .. } => {
                formatter.write_str("TypeSafe is temporarily overloaded")
            }
            Self::ProviderUnavailable => formatter.write_str("TypeSafe is unavailable"),
            Self::RequestOutcomeUnknown => {
                formatter.write_str("TypeSafe request outcome is unknown")
            }
            Self::ResponseTooLarge => {
                formatter.write_str("TypeSafe response exceeds the size limit")
            }
            Self::ResponseDecode => formatter.write_str("TypeSafe response could not be decoded"),
            Self::ResponseUsageExceeded => {
                formatter.write_str("TypeSafe response exceeded the usage limit")
            }
            Self::ProgressStorageFailure => {
                formatter.write_str("Burn Check progress could not be saved")
            }
            Self::Cancelled => formatter.write_str("Jev assessment was cancelled"),
            Self::InvalidCheckContext => formatter.write_str("Jev check context is invalid"),
            Self::InvalidCheckPlan => formatter.write_str("Jev check plan is invalid"),
        }
    }
}

impl std::error::Error for JevError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevRequest {
    pub model: String,
    pub state: Value,
    pub questions: BTreeMap<String, JevQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevQuestion {
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

impl JevQuestion {
    pub(super) fn with_context_paths(
        self,
        path: &str,
        has_shared_context: bool,
        compact: bool,
    ) -> Self {
        match self {
            Self::Choice {
                instructions,
                criteria,
            } => Self::Choice {
                instructions: contextual_instructions(
                    instructions,
                    path,
                    has_shared_context,
                    compact,
                ),
                criteria,
            },
            Self::Noul {
                instructions,
                criteria,
            } => Self::Noul {
                instructions: contextual_instructions(
                    instructions,
                    path,
                    has_shared_context,
                    compact,
                ),
                criteria,
            },
            Self::Score {
                instructions,
                criteria,
            } => Self::Score {
                instructions: contextual_instructions(
                    instructions,
                    path,
                    has_shared_context,
                    compact,
                ),
                criteria,
            },
        }
    }
}

fn contextual_instructions(
    instructions: Value,
    path: &str,
    has_shared_context: bool,
    compact: bool,
) -> Value {
    if compact {
        return if has_shared_context {
            json!({"question": instructions, "evidence": path, "shared_evidence": "shared_context"})
        } else {
            json!({"question": instructions, "evidence": path})
        };
    }
    if has_shared_context {
        json!({
            "question": instructions,
            "context_path": format!("Use the evidence at `{path}`."),
            "shared_context_path": "Use the complete shared context at `shared_context`.",
        })
    } else {
        json!({
            "question": instructions,
            "context_path": format!("Use the evidence at `{path}`."),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevResponse {
    pub model: String,
    pub answers: BTreeMap<String, JevAnswer>,
    pub usage: JevUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevAnswer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
    },
    Score {
        score: f64,
        legend: BTreeMap<String, String>,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn validate_jev_response(response: &JevResponse, request: &JevRequest) -> Result<(), JevError> {
    if response.model != request.model {
        return Err(JevError::ResponseModelMismatch);
    }
    if response.answers.len() != request.questions.len() {
        return Err(JevError::ResponseAnswerCountMismatch);
    }
    for (id, question) in &request.questions {
        let answer = response
            .answers
            .get(id)
            .ok_or(JevError::ResponseAnswerMissing)?;
        match (question, answer) {
            (
                JevQuestion::Choice { criteria, .. },
                JevAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) => {
                if !criteria.contains_key(choice)
                    || probabilities.len() != criteria.len()
                    || probabilities.keys().any(|key| !criteria.contains_key(key))
                    || probabilities
                        .values()
                        .any(|value| !valid_probability(*value))
                    || !valid_probability(*confidence)
                {
                    return Err(JevError::InvalidChoiceDistribution);
                }
                validate_probability_sum(probabilities.values().copied())?;
            }
            (JevQuestion::Noul { .. }, JevAnswer::Noul { noul }) => {
                if !valid_probability(*noul) {
                    return Err(JevError::InvalidNoulProbability);
                }
            }
            (
                JevQuestion::Score { criteria, .. },
                JevAnswer::Score {
                    score,
                    legend,
                    probabilities,
                    confidence,
                },
            ) => {
                if !score.is_finite()
                    || !valid_probability(*confidence)
                    || legend.len() != criteria.len()
                    || probabilities.len() != criteria.len()
                    || probabilities
                        .values()
                        .any(|value| !valid_probability(*value))
                {
                    return Err(JevError::InvalidScoreDistribution);
                }
                validate_probability_sum(probabilities.values().copied())?;
            }
            _ => return Err(JevError::ResponseAnswerTypeMismatch),
        }
    }
    Ok(())
}

pub fn validate_jev_response_with_capabilities(
    response: &JevResponse,
    request: &JevRequest,
    capabilities: &capabilities::ModelCapabilities,
) -> Result<(), JevError> {
    if request.model != capabilities.model
        || u32::try_from(request.questions.len()).unwrap_or(u32::MAX)
            > capabilities.questions_per_request.value.unwrap_or_default()
        || request.questions.values().any(|question| {
            let count = match question {
                JevQuestion::Choice { criteria, .. } => criteria.len(),
                JevQuestion::Score { criteria, .. } => criteria.len(),
                JevQuestion::Noul { .. } => 0,
            };
            u32::try_from(count).unwrap_or(u32::MAX)
                > capabilities.criteria_per_question.value.unwrap_or_default()
        })
    {
        return Err(JevError::QuestionLimitExceeded);
    }
    validate_jev_response(response, request)
}

/// Use the distribution to select an option when the returned choice disagrees.
pub fn highest_probability_choice<'a>(
    choice: &'a str,
    probabilities: &'a BTreeMap<String, f64>,
) -> Option<&'a str> {
    let (highest, maximum) = probabilities
        .iter()
        .max_by(|left, right| left.1.total_cmp(right.1))?;
    if probabilities
        .get(choice)
        .is_some_and(|selected| *selected + 0.000_001 >= *maximum)
    {
        Some(choice)
    } else {
        Some(highest)
    }
}

pub fn validate_jev_request(request: &JevRequest) -> Result<usize, JevError> {
    validate_jev_request_with_capabilities(request, &capabilities::ModelCapabilities::jev_default())
}

pub fn validate_jev_request_with_capabilities(
    request: &JevRequest,
    capabilities: &capabilities::ModelCapabilities,
) -> Result<usize, JevError> {
    if request.model != capabilities.model {
        return Err(JevError::UnsupportedModel);
    }
    if request.questions.is_empty() {
        return Err(JevError::EmptyQuestions);
    }
    if u32::try_from(request.questions.len()).unwrap_or(u32::MAX)
        > capabilities.questions_per_request.value.unwrap_or_default()
    {
        return Err(JevError::QuestionLimitExceeded);
    }
    if request.questions.values().any(|question| {
        let count = match question {
            JevQuestion::Choice { criteria, .. } => criteria.len(),
            JevQuestion::Score { criteria, .. } => criteria.len(),
            JevQuestion::Noul { .. } => 0,
        };
        u32::try_from(count).unwrap_or(u32::MAX)
            > capabilities.criteria_per_question.value.unwrap_or_default()
    }) {
        return Err(JevError::QuestionLimitExceeded);
    }
    let measured = json_count(request, usize::MAX).map_err(|_| JevError::RequestSerialization)?;
    let bytes = measured.bytes;
    if bytes > limit_as_usize(capabilities.request_body_bytes.value).unwrap_or_default() {
        return Err(JevError::RequestTooLarge {
            bytes,
            maximum: limit_as_usize(capabilities.request_body_bytes.value).unwrap_or_default(),
        });
    }
    let state =
        json_count(&request.state, usize::MAX).map_err(|_| JevError::RequestSerialization)?;
    let state_bytes = state.bytes;
    let mut longest_question_bytes = 0;
    let mut longest_question_tokens = 0;
    for question in request.questions.values() {
        let question =
            json_count(question, usize::MAX).map_err(|_| JevError::RequestSerialization)?;
        longest_question_bytes = longest_question_bytes.max(question.bytes);
        longest_question_tokens = longest_question_tokens
            .max(capabilities.estimated_tokens(question.estimate, question.bytes));
    }
    let state_byte_limit = limit_as_usize(capabilities.state_and_longest_question_bytes.value)
        .or_else(|| limit_as_usize(capabilities.request_body_bytes.value))
        .unwrap_or_default();
    if state_bytes.saturating_add(longest_question_bytes) > state_byte_limit {
        return Err(JevError::RequestTooLarge {
            bytes: state_bytes.saturating_add(longest_question_bytes),
            maximum: state_byte_limit,
        });
    }
    if capabilities.uses_ollama_rendered_accounting() {
        let tokens = estimate_jev_rendered_question_tokens(request, capabilities)?;
        let maximum = capabilities.usable_state_tokens().unwrap_or_default();
        if tokens > maximum {
            return Err(JevError::RequestTokenLimitExceeded { tokens, maximum });
        }
        return Ok(bytes);
    }
    let state_token_limit = capabilities.usable_state_tokens().unwrap_or_default();
    let state_tokens = capabilities
        .estimated_tokens(state.estimate, state_bytes)
        .saturating_add(longest_question_tokens);
    if state_tokens > state_token_limit {
        return Err(JevError::RequestTokenLimitExceeded {
            tokens: state_tokens,
            maximum: state_token_limit,
        });
    }
    let tokens = capabilities.estimated_tokens(measured.estimate, bytes);
    if tokens > capabilities.usable_input_tokens().unwrap_or_default() {
        return Err(JevError::RequestTokenLimitExceeded {
            tokens,
            maximum: capabilities.usable_input_tokens().unwrap_or_default(),
        });
    }
    Ok(bytes)
}

/// Estimate the largest pinned Ollama payload before the chat/scoring reserve.
/// Planners must use the packed request so that evidence paths and wrappers are counted.
pub fn estimate_jev_rendered_question_tokens(
    request: &JevRequest,
    capabilities: &capabilities::ModelCapabilities,
) -> Result<u64, JevError> {
    if !capabilities.uses_ollama_rendered_accounting() {
        return Err(JevError::UnsupportedModel);
    }
    let state = decision_content(&request.state)?;
    if capabilities.uses_ollama_generic_accounting() {
        return request.questions.keys().try_fold(0, |maximum, name| {
            let prompt = generic_question_prompt(&state, &request.questions, name)?;
            Ok(maximum.max(capabilities.estimate_text_tokens(&prompt)))
        });
    }
    request.questions.values().try_fold(0, |maximum, question| {
        let prompt = tev1_question_prompt(&state, question)?;
        Ok(maximum.max(capabilities.estimate_text_tokens(&prompt)))
    })
}

pub(super) fn decision_content(value: &Value) -> Result<String, JevError> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Object(_) | Value::Array(_) => value.to_string(),
        _ => return Err(JevError::InvalidCheckPlan),
    };
    if text.trim().is_empty() {
        return Err(JevError::InvalidCheckPlan);
    }
    Ok(text)
}

// Match Ollama v0.40.1 decision/{systemone,prompts}.go. State and object
// instructions become JSON strings inside the outer prompt.
#[derive(Serialize)]
struct DecisionOption {
    label: char,
    key: String,
    description: String,
}

fn decision_question_parts(
    question: &JevQuestion,
    tev: bool,
) -> Result<(String, Vec<DecisionOption>), JevError> {
    let (instructions, descriptions): (&Value, Vec<(String, String)>) = match question {
        JevQuestion::Choice {
            instructions,
            criteria,
        } => (
            instructions,
            criteria
                .iter()
                .map(|(key, value)| {
                    let description = match value {
                        Value::Null => key.clone(),
                        Value::String(text) => text.clone(),
                        _ => return Err(JevError::InvalidCheckPlan),
                    };
                    if key.trim().is_empty() {
                        return Err(JevError::InvalidCheckPlan);
                    }
                    Ok((key.clone(), description))
                })
                .collect::<Result<_, _>>()?,
        ),
        JevQuestion::Score {
            instructions,
            criteria,
        } => (
            instructions,
            criteria
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value
                        .as_str()
                        .map(|text| (index.to_string(), text.to_owned()))
                        .ok_or(JevError::InvalidCheckPlan)
                })
                .collect::<Result<_, _>>()?,
        ),
        JevQuestion::Noul {
            instructions,
            criteria,
        } => {
            if let Some(criteria) = criteria
                && !criteria.as_object().is_some_and(|object| {
                    object.iter().all(|(key, value)| {
                        matches!(key.as_str(), "false" | "true") && value.is_string()
                    })
                })
            {
                return Err(JevError::InvalidCheckPlan);
            }
            (
                instructions,
                [("false", "No"), ("true", "Yes")]
                    .into_iter()
                    .map(|(key, default)| {
                        (
                            key.to_owned(),
                            criteria
                                .as_ref()
                                .and_then(|value| value.get(key))
                                .and_then(Value::as_str)
                                .unwrap_or(default)
                                .to_owned(),
                        )
                    })
                    .collect(),
            )
        }
    };
    if !(2..=if tev { 24 } else { 26 }).contains(&descriptions.len())
        || (tev && descriptions.iter().any(|(_, text)| text.trim().is_empty()))
    {
        return Err(JevError::QuestionLimitExceeded);
    }
    let options = descriptions
        .into_iter()
        .zip('A'..='Z')
        .map(|((key, description), label)| DecisionOption {
            label,
            key,
            description,
        })
        .collect();
    Ok((decision_content(instructions)?, options))
}

pub(super) fn tev1_question_prompt(
    state: &str,
    question: &JevQuestion,
) -> Result<String, JevError> {
    #[derive(Serialize)]
    struct Prompt<'a> {
        state: &'a str,
        question: String,
        options: Vec<DecisionOption>,
    }
    let (question, options) = decision_question_parts(question, true)?;
    spaced_prompt_json(&Prompt {
        state,
        question,
        options,
    })
}

pub(super) fn generic_question_prompt(
    state: &str,
    questions: &BTreeMap<String, JevQuestion>,
    name: &str,
) -> Result<String, JevError> {
    #[derive(Serialize)]
    struct Choice {
        code: char,
        value: Value,
        description: String,
    }
    #[derive(Serialize)]
    struct Field {
        name: String,
        description: String,
        choices: Vec<Choice>,
    }
    #[derive(Serialize)]
    struct Prompt<'a> {
        context: &'a str,
        schema: Vec<Field>,
    }
    let schema = questions
        .iter()
        .map(|(name, question)| {
            let (description, options) = decision_question_parts(question, false)?;
            let choices = options
                .into_iter()
                .map(|option| Choice {
                    code: option.label,
                    value: if matches!(question, JevQuestion::Noul { .. }) {
                        json!(option.key == "true")
                    } else {
                        json!(option.key)
                    },
                    description: option.description,
                })
                .collect();
            Ok(Field {
                name: name.clone(),
                description,
                choices,
            })
        })
        .collect::<Result<_, JevError>>()?;
    let data = spaced_prompt_json(&Prompt {
        context: state,
        schema,
    })?;
    let name = spaced_prompt_json(&name)?;
    let escape = |text: String| text.replace('<', "\\u003c").replace('>', "\\u003e");
    Ok(format!(
        "{}\n\nRequested field: {}",
        escape(data),
        escape(name)
    ))
}

fn spaced_prompt_json(value: &impl Serialize) -> Result<String, JevError> {
    let compact = serde_json::to_string(value).map_err(|_| JevError::RequestSerialization)?;
    let mut spaced = String::with_capacity(compact.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in compact.chars() {
        spaced.push(character);
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if matches!(character, ':' | ',') {
            spaced.push(' ');
        }
    }
    Ok(spaced)
}
