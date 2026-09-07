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

/// The catalog facts resolved for one provider route and model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDefinition {
    pub canonical_model_key: String,
    pub family: ModelFamily,
    pub family_policy: FamilyPolicy,
    pub state: ModelState,
    pub effort: Support<Option<String>>,
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
#[derive(Debug, Clone)]
pub struct ReviewedModelCatalog {
    report_catalogs: ReportCatalogs,
}

impl ReviewedModelCatalog {
    pub fn new(report_catalogs: ReportCatalogs) -> Self {
        Self { report_catalogs }
    }
}

impl Default for ReviewedModelCatalog {
    fn default() -> Self {
        Self::new(ReportCatalogs::default())
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

impl ModelCatalog for ReviewedModelCatalog {
    fn resolve(&self, target: &ModelTarget) -> Support<ModelDefinition> {
        let agent = normalized_label(&target.agent);
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

        let Some(family_policy) = self.report_catalogs.families.get(&family).cloned() else {
            return unknown(CapabilityReason::PolicyUnreviewed);
        };
        let effort = resolve_effort(target.raw_effort.as_deref(), &family_policy);
        let service_tier = resolve_service_tier(&agent, target.service_tier.as_deref());
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
    ModelTarget::new(
        agent,
        provider.unwrap_or_default(),
        api.unwrap_or_default(),
        model,
    )
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

    #[test]
    fn codex_resolves_reviewed_service_tiers_and_accounting() {
        let catalog = ReviewedModelCatalog::default();
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
        let catalog = ReviewedModelCatalog::default();
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
        let catalog = ReviewedModelCatalog::default();
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
    fn opencode_effort_must_exist_in_the_resolved_family_policy() {
        let catalog = ReviewedModelCatalog::default();
        let mut target = ModelTarget::new("opencode", "anthropic", "messages", "claude-sonnet-5");
        target.raw_effort = Some("minimal".to_owned());

        let resolved = definition(catalog.resolve(&target));

        assert_eq!(
            resolved.effort,
            Support::Unknown {
                reason: CapabilityReason::UnrecognizedEffort
            }
        );
    }

    #[test]
    fn pi_effort_resolves_only_after_the_model_family_resolves() {
        let catalog = ReviewedModelCatalog::default();
        let mut target = ModelTarget::new("pi", "openai", "responses", "gpt-5.6-luna");
        target.raw_effort = Some("minimal".to_owned());

        let resolved = definition(catalog.resolve(&target));

        assert_eq!(
            resolved.effort,
            Support::Supported(Some("minimal".to_owned()))
        );
    }

    #[test]
    fn a_known_model_on_the_wrong_provider_remains_unknown() {
        let catalog = ReviewedModelCatalog::default();
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
        let catalog = ReviewedModelCatalog::default();
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
