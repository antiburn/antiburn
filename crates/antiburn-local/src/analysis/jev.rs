//! System One wire contracts and shared native content selection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod capabilities;
pub mod edit_hunks;
mod json_decode;
mod requests;

pub use json_decode::{
    JevUsageLocation, decode_jev_json_with_observation, decode_jev_json_with_usage,
};
pub use requests::{
    JevAnswer, JevError, JevQuestion, JevRequest, JevResponse, JevResponseObservation, JevUsage,
    estimate_jev_rendered_question_tokens, estimate_jev_request_tokens, highest_probability_choice,
    validate_jev_request, validate_jev_request_with_capabilities, validate_jev_response,
    validate_jev_response_with_capabilities, validate_native_jev_response_with_capabilities,
};

pub const PINNED_MODEL: &str = "jev-1.13.0";
pub const NATIVE_MODEL_ALIAS: &str = "jev-latest";
/// Bound the full serialized request before checking the returned token usage.
pub const MAX_REQUEST_BYTES: usize = 60 * 1024;
/// Use a conservative byte proxy for Jev's state-plus-question token limit.
pub const MAX_STATE_AND_LONGEST_QUESTION_BYTES: usize = 30 * 1024;
/// Jev rejects requests that exceed its current total input-token limit.
pub const MAX_REQUEST_TOKENS: u64 = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_QUESTIONS_PER_REQUEST: usize = 128;
pub const MAX_PARALLEL_REQUESTS: usize = 16;

/// One independently selectable normalized session input field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum JevInputField {
    UserMessage = 0,
    AssistantMessage = 1,
    BashCommandInput = 2,
    BashCommandOutput = 3,
    FileEditPath = 4,
    FileEditContent = 5,
    ReadFilePath = 6,
    ReadFileOutput = 7,
    SearchFilesQuery = 8,
    SearchFilesOutput = 9,
    OtherToolInput = 10,
    OtherToolOutput = 11,
    UserAnswer = 12,
    PlanReference = 13,
    ReadFileRequest = 14,
    ReadFileResult = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevFieldCapability {
    Supported,
    Conditional,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevFieldAvailabilityState {
    Excluded,
    Unsupported,
    NotObserved,
    Observed,
}

/// Bounded semantic input fields captured by the source-normalization path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNormalizedCategory {
    BashCommand,
    FileEdit,
    ReadFile,
    SearchFiles,
    OtherTool,
}

impl JevNormalizedCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BashCommand => "bash_command",
            Self::FileEdit => "file_edit",
            Self::ReadFile => "read_file",
            Self::SearchFiles => "search_files",
            Self::OtherTool => "other_tool",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bash_command" => Some(Self::BashCommand),
            "file_edit" => Some(Self::FileEdit),
            "read_file" => Some(Self::ReadFile),
            "search_files" => Some(Self::SearchFiles),
            "other_tool" => Some(Self::OtherTool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JevNormalizedFields {
    #[serde(default)]
    pub category: Option<JevNormalizedCategory>,
    pub values: BTreeMap<JevInputField, String>,
    pub malformed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JevFieldAvailability {
    pub field: JevInputField,
    pub selected: bool,
    pub capability: JevFieldCapability,
    pub state: JevFieldAvailabilityState,
    pub observed_parts: u32,
    pub empty_parts: u32,
    pub malformed_parts: u32,
    pub truncated_parts: u32,
}

/// Access to normalized session fields. An empty selection excludes all fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JevInputSelection(u16);

impl JevInputSelection {
    pub const NONE: Self = Self(0);
    /// All text fields. Structured scope records require explicit selection.
    pub const ALL: Self = Self((1 << 12) - 1);

    pub const fn from_fields(fields: &[JevInputField]) -> Self {
        let mut bits = 0u16;
        let mut index = 0;
        while index < fields.len() {
            bits |= 1 << fields[index] as u8;
            index += 1;
        }
        Self(bits)
    }

    pub const fn includes(self, field: JevInputField) -> bool {
        self.0 & (1 << field as u8) != 0
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

fn limit_as_usize(value: Option<u64>) -> Option<usize> {
    value.and_then(|value| usize::try_from(value).ok())
}

fn valid_response_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn json_count(value: &impl Serialize, maximum: usize) -> Result<JsonByteCount, serde_json::Error> {
    let mut writer = JsonByteCount {
        bytes: 0,
        maximum,
        estimate: capabilities::TokenEstimate::default(),
    };
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer)
}

struct JsonByteCount {
    bytes: usize,
    maximum: usize,
    estimate: capabilities::TokenEstimate,
}

impl std::io::Write for JsonByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("JSON byte limit exceeded"));
        }
        self.bytes += bytes.len();
        self.estimate.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn valid_probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn validate_probability_sum(values: impl Iterator<Item = f64>) -> Result<(), JevError> {
    let sum = values.sum::<f64>();
    if (sum - 1.0).abs() > 0.02 {
        Err(JevError::InvalidProbabilitySum)
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "jev/wire_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "jev/native_tests.rs"]
mod native_tests;
