//! Which provider a session's tokens were bought from.
//!
//! # The question this module answers
//!
//! "Provider" here means **who the reader pays**, not which laboratory trained
//! the weights. Those are the same thing for Claude Code (Anthropic bills the
//! Claude Code subscription or the API key behind it) and different for Cursor
//! (Cursor bills the subscription, whatever model it routes to). Attributing a
//! Cursor session's Claude tokens to Anthropic would imply money that changed
//! hands with Anthropic when it did not, so the mapping follows the billing
//! relationship.
//!
//! # Two kinds of agent
//!
//! - **Fixed route.** The agent's vendor is billed no matter which model ran:
//!   Claude Code, Codex, Copilot, Cursor, Antigravity, Windsurf, Amp, Kiro.
//!   The model evidence is not consulted, because it cannot change the answer.
//! - **Bring-your-own route.** The reader attaches their own provider account,
//!   so the *model* names the provider: Cline, OpenCode, Pi. A session that
//!   switched models mid-way is split across providers, one bucket per model.
//!
//! # What is deliberately not inferred
//!
//! A gateway (OpenRouter aside, which bills directly and is therefore named)
//! or a cloud reseller — Bedrock, Vertex, Azure — changes who is billed without
//! changing the model name, and a local transcript does not record which was
//! used. Those resolve to [`UNKNOWN`] rather than to a guess. The same applies
//! to a Claude Code install pointed at Bedrock: the fixed route says Anthropic,
//! which is the documented default, and no local evidence contradicts it.
//!
//! This conservatism is why the aggregation never emits a `live` state. Naming
//! a provider is an inference; a provider's *allowance* is not something a
//! session transcript can know at all.

use antiburn_local::model::AgentKind;

/// The provider used when the evidence does not name one.
pub const UNKNOWN: &str = "unknown";

/// Canonical provider ids. Lowercase and stable — they are dictionary keys on
/// the wire, never display copy.
pub const ANTHROPIC: &str = "anthropic";
pub const OPENAI: &str = "openai";
pub const GITHUB: &str = "github";
pub const GOOGLE: &str = "google";
pub const CURSOR: &str = "cursor";
pub const WINDSURF: &str = "windsurf";
pub const AMP: &str = "amp";
pub const KIRO: &str = "kiro";
pub const OPENROUTER: &str = "openrouter";
pub const XAI: &str = "xai";
pub const DEEPSEEK: &str = "deepseek";
pub const MISTRAL: &str = "mistral";
pub const OPENCODE: &str = "opencode";
pub const AWS: &str = "aws";
pub const AZURE: &str = "azure";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintResolution {
    Known(&'static str),
    UnknownExplicit,
}

pub fn provider_for_hint(provider: &str) -> HintResolution {
    let provider = provider.trim().to_ascii_lowercase();
    let provider = provider.replace('_', "-");
    match provider.as_str() {
        "anthropic" | "claude" => HintResolution::Known(ANTHROPIC),
        "openai" | "openai-codex" | "chatgpt" | "codex" => HintResolution::Known(OPENAI),
        "google" | "google-vertex" | "vertex" | "gemini" | "antigravity" => {
            HintResolution::Known(GOOGLE)
        }
        "github" | "github-copilot" | "copilot" => HintResolution::Known(GITHUB),
        "openrouter" => HintResolution::Known(OPENROUTER),
        "opencode" => HintResolution::Known(OPENCODE),
        "amazon-bedrock" | "aws-bedrock" | "bedrock" => HintResolution::Known(AWS),
        "azure" | "azure-openai" => HintResolution::Known(AZURE),
        "x-ai" | "xai" => HintResolution::Known(XAI),
        "deepseek" => HintResolution::Known(DEEPSEEK),
        "mistral" | "mistralai" => HintResolution::Known(MISTRAL),
        _ => HintResolution::UnknownExplicit,
    }
}

/// How an agent's tokens reach a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// One vendor is billed for every session this agent produces.
    Fixed(&'static str),
    /// The reader's own provider account is billed, so each model names its
    /// own provider.
    ByModel,
}

/// The billing route for a stored agent slug.
///
/// An unrecognized slug — a newer agent than this build knows — routes by model
/// rather than to a wrong vendor, which degrades to [`UNKNOWN`] when the model
/// is also unfamiliar.
pub fn route_for_agent(slug: &str) -> Route {
    let Some(kind) = AgentKind::from_slug(slug) else {
        return Route::ByModel;
    };
    match kind {
        // Anthropic bills Claude Code directly, through a subscription or an
        // API key. An install redirected at Bedrock or Vertex is invisible to
        // a transcript; see the module comment.
        AgentKind::Claude => Route::Fixed(ANTHROPIC),
        AgentKind::Codex => Route::Fixed(OPENAI),
        // GitHub bills Copilot, even for the Anthropic and Google models it
        // offers — the reader has no account with those vendors.
        AgentKind::Copilot => Route::Fixed(GITHUB),
        AgentKind::Cursor => Route::Fixed(CURSOR),
        AgentKind::Antigravity => Route::Fixed(GOOGLE),
        AgentKind::Windsurf => Route::Fixed(WINDSURF),
        AgentKind::AmpCode => Route::Fixed(AMP),
        AgentKind::Kiro => Route::Fixed(KIRO),
        // Bring-your-own-key clients. The reader holds the provider account,
        // so the model is the only thing that can name it.
        AgentKind::Cline | AgentKind::OpenCode | AgentKind::Pi => Route::ByModel,
    }
}

/// The provider billed for one normalized model key, for a bring-your-own
/// agent.
///
/// Two shapes appear in practice. A namespaced key (`anthropic/claude-…`)
/// names its vendor outright; a bare key (`gpt-5.6`) is matched on its family.
/// Anything else is [`UNKNOWN`]: an unrecognized name is not evidence of an
/// unrecognized *vendor*, and inventing one would put a stranger's spend under
/// somebody's account.
pub fn provider_for_model(model: &str) -> &'static str {
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() {
        return UNKNOWN;
    }
    match model.split_once('/') {
        Some((namespace, _)) => provider_for_namespace(namespace),
        None => provider_for_family(&model),
    }
}

/// The vendor a namespaced model key declares.
fn provider_for_namespace(namespace: &str) -> &'static str {
    match namespace {
        "anthropic" | "claude" => ANTHROPIC,
        "openai" | "chatgpt" => OPENAI,
        "google" | "google-vertex" | "vertex" | "gemini" => GOOGLE,
        "github" | "github-copilot" | "copilot" => GITHUB,
        // OpenRouter bills the reader itself, so it is a provider rather than
        // an unknown pass-through.
        "openrouter" => OPENROUTER,
        "x-ai" | "xai" => XAI,
        "deepseek" => DEEPSEEK,
        "mistral" | "mistralai" => MISTRAL,
        // Amazon Bedrock, Azure, and every other reseller bill someone the
        // model name cannot identify.
        _ => UNKNOWN,
    }
}

/// The vendor a bare model family belongs to.
fn provider_for_family(model: &str) -> &'static str {
    if model.starts_with("claude") {
        return ANTHROPIC;
    }
    if model.starts_with("gpt") || model.starts_with("codex") || model.starts_with("chatgpt") {
        return OPENAI;
    }
    // OpenAI's reasoning line is `o` followed by a generation digit (`o3`,
    // `o4-mini`). Narrow on purpose: `opus` and `openhands` must not match.
    if is_openai_reasoning_family(model) {
        return OPENAI;
    }
    if model.starts_with("gemini") {
        return GOOGLE;
    }
    if model.starts_with("grok") {
        return XAI;
    }
    if model.starts_with("deepseek") {
        return DEEPSEEK;
    }
    if model.starts_with("mistral") || model.starts_with("codestral") {
        return MISTRAL;
    }
    UNKNOWN
}

/// `o` immediately followed by a digit, as in `o3` or `o4-mini`.
fn is_openai_reasoning_family(model: &str) -> bool {
    let mut chars = model.chars();
    chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit())
}

/// The name a provider is shown under.
///
/// Copy, and therefore deliberately the only string in this module a reader
/// ever sees. Everything else is an identifier.
///
/// **Products, not corporations.** A reader recognizes "Claude" and "Codex";
/// they do not think of their afternoon as time spent with Anthropic. The ids
/// stay corporate (`anthropic`, `openai`) because they key attribution and
/// must not move when a product is renamed.
///
/// The imprecision this buys is worth naming: these are *provider* buckets, so
/// a bring-your-own agent pointed at an OpenAI model lands under `openai` and
/// will read as "Codex" even though Codex was never involved. The trade is
/// deliberate — the overwhelming majority of what this app sees is the vendor's
/// own agent — but if BYO usage ever becomes common, this is the line that
/// starts lying and a per-agent breakdown is the answer, not a vaguer name.
pub fn display_name(provider: &str) -> &'static str {
    match provider {
        ANTHROPIC => "Claude",
        OPENAI => "Codex",
        GITHUB => "GitHub Copilot",
        GOOGLE => "Google",
        CURSOR => "Cursor",
        WINDSURF => "Windsurf",
        AMP => "Amp",
        KIRO => "Kiro",
        OPENROUTER => "OpenRouter",
        XAI => "xAI",
        DEEPSEEK => "DeepSeek",
        MISTRAL => "Mistral",
        OPENCODE => "OpenCode",
        AWS => "AWS Bedrock",
        AZURE => "Azure OpenAI",
        _ => "Unattributed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_the_engine_models_has_a_documented_route() {
        // Exhaustive by construction — `route_for_agent` matches on the engine's
        // enum, so a new agent breaks the build rather than silently landing in
        // the unknown bucket. This pins the *choices*, which a match arm cannot.
        let fixed = [
            ("claude-code", ANTHROPIC),
            ("codex", OPENAI),
            ("copilot", GITHUB),
            ("cursor", CURSOR),
            ("antigravity", GOOGLE),
            ("windsurf", WINDSURF),
            ("amp-code", AMP),
            ("kiro", KIRO),
        ];
        for (slug, provider) in fixed {
            assert_eq!(route_for_agent(slug), Route::Fixed(provider), "{slug}");
        }
        for slug in ["cline", "opencode", "pi"] {
            assert_eq!(route_for_agent(slug), Route::ByModel, "{slug}");
        }
        assert_eq!(
            fixed.len() + 3,
            AgentKind::ALL.len(),
            "every engine agent should be named above"
        );
    }

    #[test]
    fn an_agent_this_build_does_not_know_routes_by_model_rather_than_wrongly() {
        assert_eq!(route_for_agent("some-future-agent"), Route::ByModel);
    }

    #[test]
    fn a_namespaced_model_key_names_its_own_vendor() {
        assert_eq!(provider_for_model("anthropic/claude-sonnet-4-6"), ANTHROPIC);
        assert_eq!(provider_for_model("openai/gpt-5.6"), OPENAI);
        assert_eq!(provider_for_model("google/gemini-3-pro"), GOOGLE);
        assert_eq!(provider_for_model("openrouter/auto"), OPENROUTER);
        assert_eq!(provider_for_model("x-ai/grok-4"), XAI);
    }

    #[test]
    fn explicit_provider_aliases_name_the_billing_provider() {
        assert_eq!(
            provider_for_hint("github-copilot"),
            HintResolution::Known(GITHUB)
        );
        assert_eq!(
            provider_for_hint("openai-codex"),
            HintResolution::Known(OPENAI)
        );
        assert_eq!(
            provider_for_hint("amazon-bedrock"),
            HintResolution::Known(AWS)
        );
        assert_eq!(
            provider_for_hint("azure_openai"),
            HintResolution::Known(AZURE)
        );
        assert_eq!(
            provider_for_hint("private-gateway"),
            HintResolution::UnknownExplicit
        );
    }

    #[test]
    fn a_reseller_namespace_is_unknown_rather_than_guessed() {
        // Bedrock and Azure bill AWS and Microsoft respectively, and the model
        // name cannot tell us which account is behind it.
        assert_eq!(
            provider_for_model("amazon-bedrock/claude-opus-4-6"),
            UNKNOWN
        );
        assert_eq!(provider_for_model("azure/gpt-5.6"), UNKNOWN);
    }

    #[test]
    fn a_bare_model_key_is_matched_on_its_family() {
        assert_eq!(provider_for_model("claude-opus-4-6"), ANTHROPIC);
        assert_eq!(provider_for_model("gpt-5.6"), OPENAI);
        assert_eq!(provider_for_model("o4-mini"), OPENAI);
        assert_eq!(provider_for_model("codex-mini"), OPENAI);
        assert_eq!(provider_for_model("gemini-3-pro"), GOOGLE);
        assert_eq!(provider_for_model("grok-4"), XAI);
        assert_eq!(provider_for_model("deepseek-v3"), DEEPSEEK);
        assert_eq!(provider_for_model("codestral-latest"), MISTRAL);
    }

    #[test]
    fn the_reasoning_family_match_does_not_swallow_other_o_words() {
        // The narrow rule exists so `opus` and `openhands` cannot be read as
        // OpenAI reasoning models.
        assert_eq!(provider_for_model("opus-4"), UNKNOWN);
        assert_eq!(provider_for_model("openhands-lm"), UNKNOWN);
        assert!(is_openai_reasoning_family("o3"));
        assert!(!is_openai_reasoning_family("o"));
    }

    #[test]
    fn an_unnameable_model_is_unknown_and_case_folds_first() {
        assert_eq!(provider_for_model(""), UNKNOWN);
        assert_eq!(provider_for_model("   "), UNKNOWN);
        assert_eq!(provider_for_model("some-unreleased-model"), UNKNOWN);
        assert_eq!(provider_for_model("  Claude-Opus-4-6 "), ANTHROPIC);
    }

    #[test]
    fn every_provider_id_has_display_copy_and_the_unknown_one_is_honest() {
        for provider in [
            ANTHROPIC, OPENAI, GITHUB, GOOGLE, CURSOR, WINDSURF, AMP, KIRO, OPENROUTER, XAI,
            DEEPSEEK, MISTRAL,
        ] {
            assert_ne!(display_name(provider), "Unattributed", "{provider}");
        }
        assert_eq!(display_name(UNKNOWN), "Unattributed");
    }
}
