use serde::{Deserialize, Serialize};

/// Measured heuristic, not a verified tokenizer or a universal token upper bound.
pub const ASCII_WEIGHTED_ESTIMATOR: &str = "ascii-weighted-utf8-v1";

/// Count ASCII letters at 0.75 tokens per byte and all other UTF-8 bytes at one.
/// The ASCII allowance includes a 50% margin over two letters per token.
/// Keep a separate rendering reserve for the unknown provider prompt template.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TokenEstimate {
    units: u64,
}

impl TokenEstimate {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.units = self
                .units
                .saturating_add(if byte.is_ascii_alphabetic() { 3 } else { 4 });
        }
    }

    pub(crate) fn units(self) -> u64 {
        self.units
    }

    pub(crate) fn from_units(units: u64) -> Self {
        Self { units }
    }

    pub(crate) fn tokens(self) -> u64 {
        self.units.div_ceil(4)
    }
}

/// Describes where one model capability value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    ProviderMetadata,
    RuntimeMetadata,
    DocumentedDefault,
    Manual,
    Unknown,
}

/// One capability value and its source. Unknown values stay explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityLimit<T> {
    pub value: Option<T>,
    pub source: CapabilitySource,
}

impl<T> CapabilityLimit<T> {
    pub const fn known(value: T, source: CapabilitySource) -> Self {
        Self {
            value: Some(value),
            source,
        }
    }

    pub const fn unknown() -> Self {
        Self {
            value: None,
            source: CapabilitySource::Unknown,
        }
    }
}

/// Identifies the token counting method used for a model's rendered input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum TokenizerIdentity {
    Exact(String),
    ConservativeEstimator(String),
}

/// Effective System One limits, kept separate from local queue and memory caps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Maximum evaluated input tokens across the request.
    pub total_input_tokens: CapabilityLimit<u64>,
    /// Maximum state plus the longest question, when the provider defines one.
    pub state_and_longest_question_tokens: CapabilityLimit<u64>,
    /// Optional serialized-byte guard retained separately from token limits.
    pub state_and_longest_question_bytes: CapabilityLimit<u64>,
    /// Maximum serialized request body bytes.
    pub request_body_bytes: CapabilityLimit<u64>,
    /// Maximum serialized response body bytes.
    pub response_body_bytes: CapabilityLimit<u64>,
    /// Runtime context of the loaded model, when known.
    pub runtime_context_tokens: CapabilityLimit<u64>,
    /// Maximum questions accepted in one request.
    pub questions_per_request: CapabilityLimit<u32>,
    /// Maximum criteria accepted by one choice or score question.
    pub criteria_per_question: CapabilityLimit<u32>,
    /// Tokens held back for provider rendering and scoring overhead.
    pub rendering_reserve_tokens: u64,
    /// Token counting method used during request preparation.
    pub tokenizer: Option<TokenizerIdentity>,
    /// Stable selected provider model name.
    pub model: String,
    /// Optional provider-reported digest or immutable model version.
    pub model_revision: Option<String>,
}

/// User-provided upper bounds for a custom model with incomplete metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelLimitOverride {
    pub total_input_tokens: Option<u64>,
    pub state_and_longest_question_tokens: Option<u64>,
    pub runtime_context_tokens: Option<u64>,
}

impl ModelCapabilities {
    /// Documented Jev token limits with a separate local request-memory guard.
    pub fn jev_default() -> Self {
        Self {
            total_input_tokens: CapabilityLimit::known(
                super::MAX_REQUEST_TOKENS,
                CapabilitySource::DocumentedDefault,
            ),
            state_and_longest_question_tokens: CapabilityLimit::known(
                32 * 1024,
                CapabilitySource::DocumentedDefault,
            ),
            state_and_longest_question_bytes: CapabilityLimit::unknown(),
            request_body_bytes: CapabilityLimit::known(256 * 1024, CapabilitySource::Manual),
            response_body_bytes: CapabilityLimit::known(
                super::MAX_RESPONSE_BYTES as u64,
                CapabilitySource::Manual,
            ),
            runtime_context_tokens: CapabilityLimit::unknown(),
            questions_per_request: CapabilityLimit::known(
                super::MAX_QUESTIONS_PER_REQUEST as u32,
                CapabilitySource::DocumentedDefault,
            ),
            criteria_per_question: CapabilityLimit::known(26, CapabilitySource::DocumentedDefault),
            rendering_reserve_tokens: 4 * 1024,
            tokenizer: Some(TokenizerIdentity::ConservativeEstimator(
                ASCII_WEIGHTED_ESTIMATOR.to_owned(),
            )),
            model: super::PINNED_MODEL.to_owned(),
            model_revision: None,
        }
    }

    /// Return the strictest known input-token ceiling before rendering reserve.
    pub fn usable_input_tokens(&self) -> Option<u64> {
        [
            self.total_input_tokens.value,
            self.runtime_context_tokens.value,
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|limit| limit.saturating_sub(self.rendering_reserve_tokens))
    }

    /// Apply rendering reserve to the separate state limit as well as context.
    pub fn usable_state_tokens(&self) -> Option<u64> {
        match (
            self.state_and_longest_question_tokens.value,
            self.usable_input_tokens(),
        ) {
            (Some(state), Some(total)) => Some(
                state
                    .saturating_sub(self.rendering_reserve_tokens)
                    .min(total),
            ),
            (Some(state), None) => Some(state.saturating_sub(self.rendering_reserve_tokens)),
            (None, total) => total,
        }
    }

    /// Estimate unchanged UTF-8 text. JSON escaping and question overhead need
    /// their own allowance during window construction.
    pub fn estimate_text_tokens(&self, text: &str) -> u64 {
        let mut estimate = TokenEstimate::default();
        estimate.update(text.as_bytes());
        self.estimated_tokens(estimate, text.len())
    }

    pub(crate) fn estimated_tokens(&self, estimate: TokenEstimate, bytes: usize) -> u64 {
        match &self.tokenizer {
            Some(TokenizerIdentity::ConservativeEstimator(name))
                if name == ASCII_WEIGHTED_ESTIMATOR =>
            {
                estimate.tokens()
            }
            // No exact tokenizer implementation is verified for these models.
            _ => u64::try_from(bytes).unwrap_or(u64::MAX),
        }
    }

    /// Apply lower user limits without replacing provider hard limits.
    pub fn with_override(&self, override_limits: &ModelLimitOverride) -> Self {
        let mut effective = self.clone();
        apply_lower_limit(
            &mut effective.total_input_tokens,
            override_limits.total_input_tokens,
        );
        apply_lower_limit(
            &mut effective.state_and_longest_question_tokens,
            override_limits.state_and_longest_question_tokens,
        );
        apply_lower_limit(
            &mut effective.runtime_context_tokens,
            override_limits.runtime_context_tokens,
        );
        effective
    }
}

fn apply_lower_limit(limit: &mut CapabilityLimit<u64>, override_value: Option<u64>) {
    let Some(override_value) = override_value else {
        return;
    };
    match limit.value {
        Some(known) if override_value < known => {
            limit.value = Some(override_value);
            limit.source = CapabilitySource::Manual;
        }
        None => {
            limit.value = Some(override_value);
            limit.source = CapabilitySource::Manual;
        }
        Some(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ASCII_WEIGHTED_ESTIMATOR, CapabilityLimit, CapabilitySource, ModelCapabilities,
        ModelLimitOverride, TokenEstimate, TokenizerIdentity,
    };

    #[test]
    fn estimator_characterization_keeps_margin_on_reference_tokenizers() {
        // tiktoken 0.14.0 counts plain text, not a provider request template.
        // Neither reference encoding is verified to match a System One model.
        let cases = [
            (
                "english",
                "Keep the full approved scope. Run the tests before publishing changes.\n",
                7232,
                1664,
                1664,
            ),
            (
                "code",
                "fn check(input: &str) -> bool { input.len() > 42 && input.contains(\"scope\") }\n",
                8544,
                2944,
                2944,
            ),
            (
                "cjk",
                "保留完整范围。不要删除用户批准的条件。\n",
                7424,
                2688,
                1536,
            ),
            ("emoji", "🚀👩🏽‍💻é e\u{301} 🇨🇦\n", 4448, 2944, 2176),
            (
                "arabic",
                "احتفظ بالنطاق الكامل ولا تحذف شروط الموافقة.\n",
                10496,
                3840,
                1536,
            ),
            (
                "punctuation",
                "\\\"{}[]():,;!@#$%^&*+-_=0123456789\n",
                4352,
                2304,
                2432,
            ),
        ];
        let capabilities = ModelCapabilities::jev_default();
        for (name, text, expected, cl100k, o200k) in cases {
            let text = text.repeat(128);
            let estimate = capabilities.estimate_text_tokens(&text);
            assert_eq!(estimate, expected, "{name}");
            assert!(estimate >= cl100k && estimate >= o200k, "{name}");
            println!(
                "token_estimate name={name} bytes={} estimate={estimate} cl100k={cl100k} o200k={o200k}",
                text.len()
            );
        }
    }

    #[test]
    fn streaming_estimate_is_chunk_invariant_including_split_utf8_and_escaping() {
        let text = "abZ\"\\\n漢é🚀e\u{301}123";
        let mut whole = TokenEstimate::default();
        whole.update(text.as_bytes());
        for size in 1..=text.len() {
            let mut streamed = TokenEstimate::default();
            for chunk in text.as_bytes().chunks(size) {
                streamed.update(chunk);
            }
            assert_eq!(streamed.units(), whole.units());
            assert_eq!(streamed.tokens(), whole.tokens());
        }
        assert_eq!(
            TokenEstimate::from_units(u64::MAX).tokens(),
            u64::MAX.div_ceil(4)
        );
    }

    #[test]
    fn unsupported_tokenizer_identities_keep_byte_bound_counting() {
        let mut capabilities = ModelCapabilities::jev_default();
        assert_eq!(capabilities.estimate_text_tokens("abcdefgh"), 6);
        for identity in [
            None,
            Some(TokenizerIdentity::Exact("unverified".into())),
            Some(TokenizerIdentity::ConservativeEstimator("legacy".into())),
        ] {
            capabilities.tokenizer = identity;
            assert_eq!(capabilities.estimate_text_tokens("abcdefgh"), 8);
        }
        capabilities.tokenizer = Some(TokenizerIdentity::ConservativeEstimator(
            ASCII_WEIGHTED_ESTIMATOR.into(),
        ));
        assert_eq!(capabilities.estimate_text_tokens("abcdefgh"), 6);
    }

    #[test]
    fn state_budget_applies_reserve_once_and_observes_runtime_limit() {
        let mut capabilities = ModelCapabilities::jev_default();
        assert_eq!(capabilities.usable_state_tokens(), Some(28 * 1024));
        capabilities.runtime_context_tokens =
            CapabilityLimit::known(8192, CapabilitySource::RuntimeMetadata);
        assert_eq!(capabilities.usable_state_tokens(), Some(4096));
        capabilities.state_and_longest_question_tokens =
            CapabilityLimit::known(2048, CapabilitySource::Manual);
        assert_eq!(capabilities.usable_state_tokens(), Some(0));
    }

    #[test]
    fn token_counting_profiles_bounded_large_inputs() {
        let text = "Keep approvals. 漢字 🚀\n".repeat(32 * 1024);
        let capabilities = ModelCapabilities::jev_default();
        let expected = capabilities.estimate_text_tokens(&text);
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(
                capabilities.estimate_text_tokens(std::hint::black_box(&text)),
                expected
            );
        }
        println!(
            "token_count bytes={} runs=100 elapsed_us={} estimate={expected} counter_bytes={}",
            text.len(),
            started.elapsed().as_micros(),
            std::mem::size_of::<TokenEstimate>()
        );
    }

    fn capabilities() -> ModelCapabilities {
        ModelCapabilities {
            total_input_tokens: CapabilityLimit::known(65_536, CapabilitySource::DocumentedDefault),
            state_and_longest_question_tokens: CapabilityLimit::unknown(),
            state_and_longest_question_bytes: CapabilityLimit::unknown(),
            request_body_bytes: CapabilityLimit::known(65_536, CapabilitySource::ProviderMetadata),
            response_body_bytes: CapabilityLimit::unknown(),
            runtime_context_tokens: CapabilityLimit::known(
                32_768,
                CapabilitySource::RuntimeMetadata,
            ),
            questions_per_request: CapabilityLimit::known(64, CapabilitySource::DocumentedDefault),
            criteria_per_question: CapabilityLimit::known(26, CapabilitySource::DocumentedDefault),
            rendering_reserve_tokens: 1_024,
            tokenizer: None,
            model: "clef-flash".to_owned(),
            model_revision: Some("sha256:abc".to_owned()),
        }
    }

    #[test]
    fn usable_context_uses_the_strictest_limit_and_reserve() {
        assert_eq!(capabilities().usable_input_tokens(), Some(31_744));
    }

    #[test]
    fn manual_limits_can_lower_but_not_raise_known_limits() {
        let effective = capabilities().with_override(&ModelLimitOverride {
            total_input_tokens: Some(100_000),
            state_and_longest_question_tokens: Some(8_000),
            runtime_context_tokens: Some(16_000),
        });

        assert_eq!(effective.total_input_tokens.value, Some(65_536));
        assert_eq!(
            effective.total_input_tokens.source,
            CapabilitySource::DocumentedDefault
        );
        assert_eq!(
            effective.state_and_longest_question_tokens.value,
            Some(8_000)
        );
        assert_eq!(effective.runtime_context_tokens.value, Some(16_000));
        assert_eq!(effective.usable_input_tokens(), Some(14_976));
    }

    #[test]
    fn unknown_limits_remain_unknown_without_an_override() {
        assert_eq!(CapabilityLimit::<u64>::unknown().value, None);
        assert_eq!(
            CapabilityLimit::<u64>::unknown().source,
            CapabilitySource::Unknown
        );
    }

    #[test]
    fn jev_defaults_preserve_the_pinned_model_and_token_limits() {
        let capabilities = ModelCapabilities::jev_default();

        assert_eq!(capabilities.model, super::super::PINNED_MODEL);
        assert_eq!(capabilities.total_input_tokens.value, Some(64 * 1024));
        assert_eq!(
            capabilities.state_and_longest_question_tokens.value,
            Some(32 * 1024)
        );
        assert_eq!(capabilities.request_body_bytes.value, Some(256 * 1024));
        assert_eq!(capabilities.state_and_longest_question_bytes.value, None);
        assert_eq!(capabilities.usable_state_tokens(), Some(28 * 1024));
        assert_eq!(capabilities.questions_per_request.value, Some(128));
        assert_eq!(capabilities.usable_input_tokens(), Some(60 * 1024));
    }
}
