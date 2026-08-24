// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local title summarization.
//!
//! A [`TitleSummarizer`] turns a session's first message and some early
//! context into a short display title, on device and at no cost. The engine
//! defines only the contract and the output guardrails; each platform backend
//! (for example the macOS Foundation Models sidecar) lives in the shell.
//!
//! A generated title never outranks a vendor or user title. It replaces only
//! the `firstMessage` fallback, and it is stored with its own provenance
//! (`localSummary`) so later, better sources still win.

use async_trait::async_trait;

/// What a summarizer needs to name one session. Serializes as the JSON a
/// sidecar backend writes to the helper's stdin.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleInput {
    /// Repository or directory name, when known. Anchors the title.
    pub repo: Option<String>,
    /// The user's first message, bounded upstream.
    pub first_message: String,
    /// A few later user messages, bounded upstream. More context makes a
    /// materially better title than the first message alone.
    pub context: Vec<String>,
}

/// Whether a backend can run right now. Availability changes at run time
/// (for example the user turns Apple Intelligence off), so callers probe per
/// pass and never cache the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummarizerAvailability {
    Available,
    /// Human-readable reason, for diagnostics only.
    Unavailable(String),
}

/// A local, on-device title generator. Implementations must not call any
/// remote service and must not spend API tokens.
#[async_trait]
pub trait TitleSummarizer: Send + Sync {
    async fn availability(&self) -> SummarizerAvailability;

    /// Return a short title for `input`, or `None` when generation fails.
    /// Callers pass the result through [`sanitize_generated_title`]; a `None`
    /// keeps the existing fallback title.
    async fn title(&self, input: &TitleInput) -> Option<String>;
}

/// Longest sanitized generated title, in characters. Matches the cleaned
/// first-message budget so both render the same in one activity row.
const GENERATED_TITLE_MAX_CHARS: usize = 60;

/// Openers that mark a refusal rather than a title. Model output is
/// untrusted; a refusal must never become a session name.
const REFUSAL_PREFIXES: &[&str] = &["i can't", "i cannot", "i'm sorry", "sorry", "unable to"];

/// Guardrails for model output: keep the first line, remove wrapping quotes,
/// collapse whitespace, drop a trailing period, capitalize the first letter,
/// and cap the length at a word boundary. Returns `None` for empty or
/// refusal-looking output, so the caller keeps the fallback title instead.
pub fn sanitize_generated_title(raw: &str) -> Option<String> {
    let first_line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let unquoted = first_line
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '\u{201c}' | '\u{201d}'))
        .trim();
    let collapsed: String = unquoted.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches('.').trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    if REFUSAL_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
    {
        return None;
    }
    let capitalized = capitalize_first(trimmed);
    let chars: Vec<char> = capitalized.chars().collect();
    if chars.len() <= GENERATED_TITLE_MAX_CHARS {
        return Some(capitalized);
    }
    let head: String = chars[..GENERATED_TITLE_MAX_CHARS].iter().collect();
    let cut = match head.rfind(char::is_whitespace) {
        Some(at) if at > 0 => &head[..at],
        _ => head.as_str(),
    };
    Some(format!("{}…", cut.trim_end()))
}

/// Uppercase the first letter of `text`. Only a lowercase first letter
/// changes; `to_uppercase` handles multi-character expansions.
pub(crate) fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_quotes_and_trailing_period() {
        assert_eq!(
            sanitize_generated_title("\"Fix login redirect loop.\"").as_deref(),
            Some("Fix login redirect loop")
        );
        assert_eq!(
            sanitize_generated_title("`Review project status`\nSecond line ignored").as_deref(),
            Some("Review project status")
        );
    }

    #[test]
    fn sanitize_rejects_empty_and_refusals() {
        assert!(sanitize_generated_title("   ").is_none());
        assert!(sanitize_generated_title("\"\"").is_none());
        assert!(sanitize_generated_title("I'm sorry, I can't name this chat").is_none());
        assert!(sanitize_generated_title("Sorry, there is no content").is_none());
    }

    #[test]
    fn sanitize_capitalizes_the_first_letter() {
        assert_eq!(
            sanitize_generated_title("examine cadence-cli data ingestion").as_deref(),
            Some("Examine cadence-cli data ingestion")
        );
        // A first letter that is not a lowercase letter stays as it is.
        assert_eq!(
            sanitize_generated_title("3-D print a fridge shelf").as_deref(),
            Some("3-D print a fridge shelf")
        );
    }

    #[test]
    fn sanitize_caps_length_at_word_boundary() {
        let long = "Rework the discovery scanner title pipeline for every supported agent kind";
        let sanitized = sanitize_generated_title(long).unwrap();
        assert!(sanitized.ends_with('…'));
        assert!(sanitized.chars().count() <= 61);
        assert!(
            !sanitized
                .trim_end_matches('…')
                .ends_with(char::is_whitespace)
        );
    }
}
