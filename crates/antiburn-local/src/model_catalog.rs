//! Reviewed model capabilities for local analysis.

use crate::analysis::{RepeatedContextAccounting, lookup_pricing};
use crate::insights::{FamilyPolicy, ModelFamily, ModelReplacementEntry, ReportCatalogs};
use crate::pricing::{ModelPricing, canonical_model_key};

/// The result of resolving a capability or reviewed fact.
#[derive(Debug, Clone, PartialEq)]
pub enum Support<T> {
    Supported(T),
    Unsupported { reason: CapabilityReason },
    Unknown { reason: CapabilityReason },
}

/// The reason a catalog cannot return a supported value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityReason {
    MissingProvider,
    UnknownProvider,
    MissingApi,
    UnknownApi,
    UnknownModel,
    ProviderModelMismatch,
    ApiModelMismatch,
    UnsupportedAgent,
    NotApplicable,
    UnrecognizedEffort,
    UnrecognizedServiceTier,
    PricingUnavailable,
    PolicyUnreviewed,
}

/// The provider route, model, and optional native values to resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTarget {
    pub agent: String,
    pub provider: String,
    pub api: String,
    pub model: String,
    pub raw_effort: Option<String>,
    pub service_tier: Option<String>,
}

impl ModelTarget {
    pub fn new(
        agent: impl Into<String>,
        provider: impl Into<String>,
        api: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            agent: agent.into(),
            provider: provider.into(),
            api: api.into(),
            model: model.into(),
            raw_effort: None,
            service_tier: None,
        }
    }
}

/// The reviewed lifecycle state of a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    Current,
    Obsolete(ModelReplacementEntry),
}

/// The meaning of the resolved effort value, not a provider translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortSemantics {
    ProviderEffort,
    AgentSelectedPolicy,
}

/// The catalog facts resolved for one provider route and model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDefinition {
    pub canonical_model_key: String,
    pub family: ModelFamily,
    pub family_policy: FamilyPolicy,
    pub state: ModelState,
    pub effort: Support<Option<String>>,
    pub effort_semantics: EffortSemantics,
    pub service_tier: Support<Option<String>>,
    pub pricing: Support<ModelPricing>,
    pub repeated_context_accounting: Support<RepeatedContextAccounting>,
    pub catalog_revision: i64,
    pub replacement_registry_revision: u32,
}

/// Resolves model facts without provider I/O.
pub trait ModelCatalog: Send + Sync {
    fn resolve(&self, target: &ModelTarget) -> Support<ModelDefinition>;
}

/// The compiled catalog of maintainer-reviewed routes and model states.
#[derive(Debug, Clone, Copy)]
pub struct ReviewedModelCatalog<'a> {
    report_catalogs: &'a ReportCatalogs,
}

impl<'a> ReviewedModelCatalog<'a> {
    pub fn new(report_catalogs: &'a ReportCatalogs) -> Self {
        Self { report_catalogs }
    }
}

impl Default for ReviewedModelCatalog<'static> {
    fn default() -> Self {
        static CATALOGS: std::sync::OnceLock<ReportCatalogs> = std::sync::OnceLock::new();
        Self::new(CATALOGS.get_or_init(ReportCatalogs::default))
    }
}

#[derive(Clone, Copy)]
struct Route {
    agent: &'static str,
    provider: &'static str,
    api: &'static str,
    family: ModelFamily,
}

const ROUTES: &[Route] = &[
    Route {
        agent: "codex",
        provider: "openai",
        api: "responses",
        family: ModelFamily::OpenAi,
    },
    Route {
        agent: "claude",
        provider: "anthropic",
        api: "messages",
        family: ModelFamily::Claude,
    },
    Route {
        agent: "opencode",
        provider: "openai",
        api: "responses",
        family: ModelFamily::OpenAi,
    },
    Route {
        agent: "opencode",
        provider: "anthropic",
        api: "messages",
        family: ModelFamily::Claude,
    },
    Route {
        agent: "opencode",
        provider: "google",
        api: "generate-content",
        family: ModelFamily::Google,
    },
    Route {
        agent: "pi",
        provider: "openai",
        api: "responses",
        family: ModelFamily::OpenAi,
    },
    Route {
        agent: "pi",
        provider: "openai",
        api: "openai-responses",
        family: ModelFamily::OpenAi,
    },
    Route {
        agent: "pi",
        provider: "openai-codex",
        api: "openai-codex-responses",
        family: ModelFamily::OpenAi,
    },
    Route {
        agent: "pi",
        provider: "openai",
        api: "openai-completions",
        family: ModelFamily::OpenAi,
    },
    Route {
        agent: "pi",
        provider: "anthropic",
        api: "messages",
        family: ModelFamily::Claude,
    },
    Route {
        agent: "pi",
        provider: "anthropic",
        api: "anthropic-messages",
        family: ModelFamily::Claude,
    },
    Route {
        agent: "pi",
        provider: "google",
        api: "generate-content",
        family: ModelFamily::Google,
    },
    Route {
        agent: "pi",
        provider: "google",
        api: "google-generative-ai",
        family: ModelFamily::Google,
    },
];

const CURRENT_MODELS: &[(&str, ModelFamily)] = &[
    ("claude-opus-5", ModelFamily::Claude),
    ("claude-sonnet-5", ModelFamily::Claude),
    ("claude-haiku-4-5", ModelFamily::Claude),
    ("claude-fable-5", ModelFamily::Claude),
    ("gpt-5.6", ModelFamily::OpenAi),
    ("gpt-5.6-sol", ModelFamily::OpenAi),
    ("gpt-5.6-luna", ModelFamily::OpenAi),
    ("gpt-6-astra", ModelFamily::OpenAi),
    ("gemini-3.8-pro", ModelFamily::Google),
    ("gemini-3.8-flash", ModelFamily::Google),
];

/// Returns a reviewed lifecycle state without guessing a provider route.
pub fn reviewed_model_state(
    registry: &crate::insights::ModelRegistry,
    model: &str,
) -> Support<ModelState> {
    let canonical = canonical_model_key(model);
    if let Some(entry) = registry.lookup(&canonical) {
        return Support::Supported(ModelState::Obsolete(entry.clone()));
    }
    if registry
        .entries
        .values()
        .any(|entry| canonical_model_key(&entry.replacement) == canonical)
        || CURRENT_MODELS
            .iter()
            .any(|(current, _)| *current == canonical)
    {
        Support::Supported(ModelState::Current)
    } else {
        unknown(CapabilityReason::UnknownModel)
    }
}

impl ModelCatalog for ReviewedModelCatalog<'_> {
    fn resolve(&self, target: &ModelTarget) -> Support<ModelDefinition> {
        let agent = normalized_label(&target.agent);
        let agent = if agent == "claude-code" {
            "claude"
        } else {
            agent.as_str()
        };
        let provider = normalized_label(&target.provider);
        let api = normalized_label(&target.api);
        if provider.is_empty() {
            return unknown(CapabilityReason::MissingProvider);
        }
        if api.is_empty() {
            return unknown(CapabilityReason::MissingApi);
        }

        let provider_known = ROUTES.iter().any(|route| route.provider == provider);
        if !provider_known {
            return unknown(CapabilityReason::UnknownProvider);
        }
        let api_known = ROUTES.iter().any(|route| route.api == api);
        if !api_known {
            return unknown(CapabilityReason::UnknownApi);
        }
        if !ROUTES.iter().any(|route| route.agent == agent) {
            return unknown(CapabilityReason::UnsupportedAgent);
        }

        let Some(route) = ROUTES
            .iter()
            .find(|route| route.agent == agent && route.provider == provider && route.api == api)
        else {
            let provider_route = ROUTES
                .iter()
                .any(|route| route.agent == agent && route.provider == provider);
            return unknown(if provider_route {
                CapabilityReason::ApiModelMismatch
            } else {
                CapabilityReason::ProviderModelMismatch
            });
        };

        let canonical = canonical_model_key(&target.model);
        let Support::Supported(state) =
            reviewed_model_state(&self.report_catalogs.model_replacements, &canonical)
        else {
            return unknown(CapabilityReason::UnknownModel);
        };
        let family = crate::insights::model_family(&canonical);
        if family != route.family {
            return unknown(CapabilityReason::ProviderModelMismatch);
        }

        let (family_policy, effort_semantics) = if agent == "pi" {
            let mut policy = self
                .report_catalogs
                .families
                .get(&family)
                .cloned()
                .unwrap_or_default();
            // Pi selects an ordered policy level. Model maps and request overrides can change the provider effort.
            policy.effort.recognized = ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            policy.effort.above_cap = ["xhigh", "max"].into_iter().map(str::to_owned).collect();
            (policy, EffortSemantics::AgentSelectedPolicy)
        } else if let Some(policy) = self.report_catalogs.families.get(&family).cloned() {
            (policy, EffortSemantics::ProviderEffort)
        } else {
            return unknown(CapabilityReason::PolicyUnreviewed);
        };
        let effort = if agent == "opencode" {
            Support::Unsupported {
                reason: CapabilityReason::PolicyUnreviewed,
            }
        } else {
            resolve_effort(target.raw_effort.as_deref(), &family_policy)
        };
        let service_tier = resolve_service_tier(agent, target.service_tier.as_deref());
        let pricing = match lookup_pricing(&canonical) {
            Some(pricing) => Support::Supported(pricing),
            None => unknown(CapabilityReason::PricingUnavailable),
        };
        let repeated_context_accounting = match family {
            ModelFamily::Claude => Support::Supported(RepeatedContextAccounting::CacheWrite),
            ModelFamily::OpenAi => Support::Supported(RepeatedContextAccounting::UncachedInput),
            ModelFamily::Google | ModelFamily::Unknown => {
                unknown(CapabilityReason::PolicyUnreviewed)
            }
        };

        Support::Supported(ModelDefinition {
            canonical_model_key: canonical,
            family,
            family_policy,
            state,
            effort,
            effort_semantics,
            service_tier,
            pricing,
            repeated_context_accounting,
            catalog_revision: self.report_catalogs.revision,
            replacement_registry_revision: self.report_catalogs.model_replacements.revision,
        })
    }
}

fn resolve_effort(raw: Option<&str>, policy: &FamilyPolicy) -> Support<Option<String>> {
    let Some(raw) = raw else {
        return Support::Supported(None);
    };
    let effort = normalized_label(raw);
    if policy.effort.recognized.contains(effort.as_str()) {
        Support::Supported(Some(effort))
    } else {
        unknown(CapabilityReason::UnrecognizedEffort)
    }
}

fn resolve_service_tier(agent: &str, raw: Option<&str>) -> Support<Option<String>> {
    if agent != "codex" && agent != "claude" {
        return Support::Unsupported {
            reason: CapabilityReason::NotApplicable,
        };
    }
    let Some(raw) = raw else {
        return Support::Supported(None);
    };
    match normalized_label(raw).as_str() {
        "priority" | "fast" => Support::Supported(Some("fast".to_owned())),
        "default" | "standard" => Support::Supported(Some("standard".to_owned())),
        _ => unknown(CapabilityReason::UnrecognizedServiceTier),
    }
}

/// Builds a target only when the source has one reviewed fixed provider route.
pub fn fixed_route_target(agent: &str, model: &str) -> Option<ModelTarget> {
    match normalized_label(agent).as_str() {
        "claude" | "claude-code" => {
            Some(ModelTarget::new("claude", "anthropic", "messages", model))
        }
        "codex" => Some(ModelTarget::new("codex", "openai", "responses", model)),
        _ => None,
    }
}

pub fn model_control_target(
    agent: &str,
    provider: Option<&str>,
    api: Option<&str>,
    model: &str,
) -> ModelTarget {
    if provider.is_none()
        && api.is_none()
        && let Some(target) = fixed_route_target(agent, model)
    {
        return target;
    }
    if normalized_label(agent) == "opencode"
        && let (Some(provider), None) = (provider, api)
        && let Some(api) = opencode_direct_provider_api(provider)
    {
        return ModelTarget::new(agent, provider, api, model);
    }
    ModelTarget::new(
        agent,
        provider.unwrap_or_default(),
        api.unwrap_or_default(),
        model,
    )
}

/// OpenCode stores direct provider IDs but not a request API discriminator.
/// These reviewed providers each have one native API route in OpenCode.
fn opencode_direct_provider_api(provider: &str) -> Option<&'static str> {
    match normalized_label(provider).as_str() {
        "openai" => Some("responses"),
        "anthropic" => Some("messages"),
        "google" => Some("generate-content"),
        _ => None,
    }
}

fn normalized_label(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn unknown<T>(reason: CapabilityReason) -> Support<T> {
    Support::Unknown { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(result: Support<ModelDefinition>) -> ModelDefinition {
        match result {
            Support::Supported(definition) => definition,
            other => panic!("expected a model definition, got {other:?}"),
        }
    }

    fn default_catalogs() -> ReportCatalogs {
        ReportCatalogs::default()
    }

    #[test]
    fn codex_resolves_reviewed_service_tiers_and_accounting() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let mut target = ModelTarget::new("codex", "openai", "responses", "GPT-5.6-SOL");
        target.raw_effort = Some(" High ".to_owned());
        target.service_tier = Some("priority".to_owned());

        let resolved = definition(catalog.resolve(&target));

        assert_eq!(resolved.canonical_model_key, "gpt-5.6-sol");
        assert_eq!(resolved.state, ModelState::Current);
        assert_eq!(resolved.effort, Support::Supported(Some("high".to_owned())));
        assert_eq!(
            resolved.service_tier,
            Support::Supported(Some("fast".to_owned()))
        );
        assert_eq!(
            resolved.repeated_context_accounting,
            Support::Supported(RepeatedContextAccounting::UncachedInput)
        );
    }

    #[test]
    fn unknown_models_do_not_become_current_from_their_prefix() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let target = ModelTarget::new("codex", "openai", "responses", "gpt-unreviewed");

        assert_eq!(
            catalog.resolve(&target),
            Support::Unknown {
                reason: CapabilityReason::UnknownModel
            }
        );
    }

    #[test]
    fn obsolete_models_reuse_the_reviewed_replacement_registry() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let target = ModelTarget::new("claude", "anthropic", "messages", "claude-sonnet-4.6");

        let resolved = definition(catalog.resolve(&target));
        let ModelState::Obsolete(replacement) = resolved.state else {
            panic!("expected an obsolete model");
        };
        assert_eq!(replacement.replacement, "claude-sonnet-5");
    }

    #[test]
    fn the_claude_discovery_slug_uses_the_reviewed_fixed_route() {
        assert_eq!(
            fixed_route_target("claude-code", "claude-sonnet-5"),
            Some(ModelTarget::new(
                "claude",
                "anthropic",
                "messages",
                "claude-sonnet-5"
            ))
        );
    }

    #[test]
    fn the_claude_discovery_slug_resolves_an_explicit_reviewed_route() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let mut target = model_control_target(
            " Claude-Code ",
            Some("anthropic"),
            Some("messages"),
            "claude-sonnet-5",
        );
        target.raw_effort = Some("max".to_owned());
        target.service_tier = Some("fast".to_owned());
        let resolved = definition(catalog.resolve(&target));
        assert_eq!(resolved.effort, Support::Supported(Some("max".to_owned())));
        assert_eq!(
            resolved.service_tier,
            Support::Supported(Some("fast".to_owned()))
        );
    }

    #[test]
    fn explicit_or_incomplete_routes_do_not_fall_back_to_the_fixed_route() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        for agent in ["claude", "claude-code", "codex"] {
            let model = if agent == "codex" {
                "gpt-5.6"
            } else {
                "claude-sonnet-5"
            };
            for (provider, api, reason) in [
                (
                    Some("gateway"),
                    Some("messages"),
                    CapabilityReason::UnknownProvider,
                ),
                (None, Some("messages"), CapabilityReason::MissingProvider),
                (Some("anthropic"), None, CapabilityReason::MissingApi),
                (Some(""), Some(""), CapabilityReason::MissingProvider),
            ] {
                assert_eq!(
                    catalog.resolve(&model_control_target(agent, provider, api, model)),
                    Support::Unknown { reason },
                    "{agent}: {provider:?}/{api:?}"
                );
            }
        }
        for agent in ["opencode", "pi", "antigravity"] {
            assert_eq!(fixed_route_target(agent, "claude-sonnet-5"), None);
        }
    }

    #[test]
    fn opencode_variants_do_not_resolve_as_family_effort() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        for (provider, api, model) in [
            ("anthropic", "messages", "claude-sonnet-5"),
            ("openai", "responses", "gpt-5.6"),
            ("google", "generate-content", "gemini-3.8-pro"),
        ] {
            for variant in [
                None,
                Some("high"),
                Some("xhigh"),
                Some("max"),
                Some("custom"),
            ] {
                let mut target = ModelTarget::new("opencode", provider, api, model);
                target.raw_effort = variant.map(str::to_owned);
                let resolved = definition(catalog.resolve(&target));
                assert_eq!(resolved.canonical_model_key, model);
                assert_eq!(resolved.state, ModelState::Current);
                assert_eq!(
                    resolved.effort,
                    Support::Unsupported {
                        reason: CapabilityReason::PolicyUnreviewed
                    }
                );
            }
        }
    }

    #[test]
    fn opencode_direct_providers_supply_their_reviewed_native_api_only() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        for (provider, api, model) in [
            ("openai", "responses", "gpt-5.6"),
            ("anthropic", "messages", "claude-sonnet-5"),
            ("google", "generate-content", "gemini-3.8-pro"),
        ] {
            let target = model_control_target("opencode", Some(provider), None, model);
            assert_eq!(target.api, api);
            assert!(matches!(catalog.resolve(&target), Support::Supported(_)));
        }
        for provider in ["opencode", "github-copilot", "bedrock", "custom-proxy"] {
            let target = model_control_target("opencode", Some(provider), None, "gpt-5.6");
            assert!(target.api.is_empty());
            assert!(!matches!(catalog.resolve(&target), Support::Supported(_)));
        }
    }

    #[test]
    fn pi_effort_resolves_only_after_the_route_and_model_resolve() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let mut target = ModelTarget::new("pi", "openai", "responses", "gpt-5.6-luna");
        target.raw_effort = Some("minimal".to_owned());

        let resolved = definition(catalog.resolve(&target));

        assert_eq!(
            resolved.effort,
            Support::Supported(Some("minimal".to_owned()))
        );
    }

    // Reviewed upstream: https://github.com/badlogic/pi-mono/tree/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/ai/src
    // types.ts defines policy levels. models.ts clamps them. api/openai-responses.ts applies thinkingLevelMap.
    // providers/openai.ts, openai-codex.ts, and google.ts declare the native provider/API pairs.
    // api/google-generative-ai.ts maps medium to HIGH for Gemini Pro and permits custom token budgets.
    #[test]
    fn pi_native_routes_preserve_agent_policy_without_provider_translation() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        for (provider, api, model) in [
            ("openai", "openai-responses", "gpt-5.6"),
            ("openai-codex", "openai-codex-responses", "gpt-5.6"),
            ("openai", "responses", "gpt-5.6"),
            ("openai", "openai-completions", "gpt-5.6"),
            ("anthropic", "anthropic-messages", "claude-sonnet-4.6"),
            ("anthropic", "messages", "claude-sonnet-4.6"),
            ("google", "google-generative-ai", "gemini-3.8-pro"),
            ("google", "generate-content", "gemini-3.8-pro"),
        ] {
            for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
                let mut target = ModelTarget::new("pi", provider, api, model);
                target.raw_effort = Some(level.to_owned());
                let resolved = definition(catalog.resolve(&target));
                assert_eq!(
                    resolved.effort_semantics,
                    EffortSemantics::AgentSelectedPolicy
                );
                assert_eq!(resolved.effort, Support::Supported(Some(level.to_owned())));
                assert_eq!(
                    resolved.family_policy.effort.above_cap.contains(level),
                    matches!(level, "xhigh" | "max")
                );
            }
            for level in ["none", "ultra", "32768", "adaptive", "unreviewed"] {
                let mut target = ModelTarget::new("pi", provider, api, model);
                target.raw_effort = Some(level.to_owned());
                assert_eq!(
                    definition(catalog.resolve(&target)).effort,
                    unknown(CapabilityReason::UnrecognizedEffort)
                );
            }
        }
    }

    #[test]
    fn pi_native_api_names_do_not_authorize_other_providers_or_models() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        for (provider, api, model, reason) in [
            (
                "",
                "openai-responses",
                "gpt-5.6",
                CapabilityReason::MissingProvider,
            ),
            ("openai", "", "gpt-5.6", CapabilityReason::MissingApi),
            (
                "gateway",
                "openai-responses",
                "gpt-5.6",
                CapabilityReason::UnknownProvider,
            ),
            (
                "openai",
                "google-generative-ai",
                "gpt-5.6",
                CapabilityReason::ApiModelMismatch,
            ),
            (
                "google",
                "google-generative-ai",
                "gpt-5.6",
                CapabilityReason::ProviderModelMismatch,
            ),
            (
                "google",
                "google-generative-ai",
                "gemini-unreviewed",
                CapabilityReason::UnknownModel,
            ),
        ] {
            assert_eq!(
                catalog.resolve(&ModelTarget::new("pi", provider, api, model)),
                unknown(reason)
            );
        }
    }

    #[test]
    fn a_known_model_on_the_wrong_provider_remains_unknown() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let target = ModelTarget::new("pi", "openai", "responses", "claude-sonnet-5");

        assert_eq!(
            catalog.resolve(&target),
            Support::Unknown {
                reason: CapabilityReason::ProviderModelMismatch
            }
        );
    }

    #[test]
    fn an_unreviewed_codex_service_tier_remains_unknown() {
        let catalogs = default_catalogs();
        let catalog = ReviewedModelCatalog::new(&catalogs);
        let mut target = ModelTarget::new("codex", "openai", "responses", "gpt-5.6");
        target.service_tier = Some("economy".to_owned());

        let resolved = definition(catalog.resolve(&target));

        assert_eq!(
            resolved.service_tier,
            Support::Unknown {
                reason: CapabilityReason::UnrecognizedServiceTier
            }
        );
    }
}
