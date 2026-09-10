//! Shell policy for the passive popover peek anchored companion window.

use std::time::Duration;

use antiburn_anchored_window::{
    AnchorRegion, AnchoredWindowConfig, AnchoredWindowManager, AnchoredWindowRequest,
    AnchoredWindowState, PointerExitPolicy,
};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::dto::{LiveUsageSummary, ProviderUsageSummary};

pub const LABEL: &str = "popover-peek";
const HOVER_LEAVE_GRACE: Duration = Duration::from_millis(300);

/// The data identity a popover hover asks the companion to preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PopoverPeekTarget {
    Provider {
        provider: String,
        utc_offset_minutes: i32,
    },
    Checks,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecksCategory {
    id: String,
    finding: u64,
    clean: u64,
    unavailable: u64,
    estimated_token_burn_basis_points: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecksEstimate {
    token_burn_basis_points: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChecksPresentation {
    failures: Vec<ChecksCategory>,
    wins: Vec<ChecksCategory>,
    unavailable: Vec<ChecksCategory>,
    refresh_unavailable: bool,
    estimate: ChecksEstimate,
}

/// Fresh data returned only to the current companion generation.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PopoverPeekData {
    Provider {
        summary: Box<ProviderUsageSummary>,
        live: Box<LiveUsageSummary>,
    },
    Checks {
        presentation: ChecksPresentation,
    },
}

pub type PopoverPeekManager = AnchoredWindowManager<PopoverPeekTarget, PopoverPeekData>;

/// Tallest the anchored Usage preview may get, in logical pixels.
const MAX_CONTENT_HEIGHT: f64 = 780.0;

pub fn manager() -> PopoverPeekManager {
    let manager = AnchoredWindowManager::new(AnchoredWindowConfig {
        label: LABEL.to_string(),
        anchor_label: crate::popover::LABEL.to_string(),
        route: if cfg!(target_os = "macos") {
            "native-peek.html"
        } else {
            "index.html#/popover-peek"
        }
        .to_string(),
        title: "antiburn".to_string(),
        width: 380.0,
        corner_radius: crate::popover::CORNER_RADIUS,
        initial_height: 320.0,
        min_height: 60.0,
        max_height: MAX_CONTENT_HEIGHT,
        gap: 8.0,
        screen_margin: 8.0,
        conceal_fallback: Duration::from_millis(80),
        pointer_exit: Some(PointerExitPolicy {
            edge_tolerance: 12.0,
            outside_delay: Duration::from_millis(180),
        }),
    });
    #[cfg(target_os = "macos")]
    let manager = manager.with_native_handler(native_request);
    manager
}

fn validate_popover_caller(actual: &str) -> Result<(), String> {
    if actual == crate::popover::LABEL {
        Ok(())
    } else {
        Err(format!("{actual} cannot control {LABEL}"))
    }
}

fn validate_peek_caller(actual: &str) -> Result<(), String> {
    if actual == LABEL {
        Ok(())
    } else {
        Err(format!("{actual} cannot read or acknowledge {LABEL}"))
    }
}

fn validate_state_caller(actual: &str) -> Result<(), String> {
    if actual == LABEL || actual == crate::popover::LABEL {
        Ok(())
    } else {
        Err(format!("{actual} cannot read {LABEL} state"))
    }
}

#[tauri::command]
pub fn show_popover_peek(
    window: tauri::WebviewWindow,
    target: PopoverPeekTarget,
    anchor: AnchorRegion,
    initial_presentation: Option<PopoverPeekData>,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<AnchoredWindowRequest<PopoverPeekTarget>, String> {
    validate_popover_caller(window.label())?;
    let presentation = match initial_presentation {
        Some(presentation) => Some(match (&target, presentation) {
            (
                PopoverPeekTarget::Provider { provider, .. },
                PopoverPeekData::Provider { summary, live },
            ) => selected_provider_data(provider, *summary, *live),
            (PopoverPeekTarget::Checks, PopoverPeekData::Checks { presentation }) => {
                validate_checks_presentation(&presentation)?;
                PopoverPeekData::Checks { presentation }
            }
            _ => return Err("popover peek presentation does not match its target".to_string()),
        }),
        None => None,
    };
    manager
        .request_with_presentation(window.app_handle(), target, anchor, presentation)
        .map_err(|error| error.to_string())
}

fn validate_checks_presentation(presentation: &ChecksPresentation) -> Result<(), String> {
    if presentation.failures.len() + presentation.wins.len() + presentation.unavailable.len() > 9 {
        return Err("checks preview has too many categories".to_string());
    }
    let mut ids = std::collections::HashSet::new();
    for (category, section) in presentation
        .failures
        .iter()
        .map(|category| (category, "failures"))
        .chain(presentation.wins.iter().map(|category| (category, "wins")))
        .chain(
            presentation
                .unavailable
                .iter()
                .map(|category| (category, "unavailable")),
        )
    {
        if !matches!(
            category.id.as_str(),
            "sessionsOverDepth"
                | "modelOverthinking"
                | "overpoweredSubagents"
                | "unusedMcpServers"
                | "unusedBuiltInTools"
                | "unusedSkills"
                | "oldModelUsage"
                | "overuseOfFastMode"
                | "cacheChurn"
        ) || !ids.insert(category.id.as_str())
            || category
                .estimated_token_burn_basis_points
                .is_some_and(|value| {
                    value > antiburn_local::insights::MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS
                })
            || (section == "failures" && category.finding == 0)
            || (section == "wins" && (category.finding != 0 || category.clean == 0))
            || (section == "unavailable"
                && (category.finding != 0
                    || category.clean != 0
                    || category.estimated_token_burn_basis_points.is_some()))
        {
            return Err("checks preview category is invalid".to_string());
        }
    }
    if presentation
        .estimate
        .token_burn_basis_points
        .is_some_and(|value| {
            value > antiburn_local::insights::MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS
        })
    {
        return Err("checks preview estimate is invalid".to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn hide_popover_peek(
    window: tauri::WebviewWindow,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<(), String> {
    validate_popover_caller(window.label())?;
    manager.conceal_after(window.app_handle(), HOVER_LEAVE_GRACE);
    Ok(())
}

#[tauri::command]
pub fn get_popover_peek_state(
    window: tauri::WebviewWindow,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<AnchoredWindowState<PopoverPeekTarget>, String> {
    validate_state_caller(window.label())?;
    Ok(manager.state())
}

#[tauri::command]
pub fn get_popover_peek_data(
    window: tauri::WebviewWindow,
    generation: u64,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<PopoverPeekData, String> {
    validate_peek_caller(window.label())?;
    peek_data(window.app_handle(), &manager, generation)
}

fn peek_data(
    app: &tauri::AppHandle,
    manager: &PopoverPeekManager,
    generation: u64,
) -> Result<PopoverPeekData, String> {
    let target = current_target(manager, generation)?;
    match target {
        PopoverPeekTarget::Provider {
            provider,
            utc_offset_minutes,
        } => {
            let local = crate::commands::provider_usage_summary(app, Some(utc_offset_minutes))?;
            let live = crate::commands::cached_live_usage(app);
            current_target(manager, generation)?;
            Ok(selected_provider_data(&provider, local, live))
        }
        PopoverPeekTarget::Checks => {
            Err("checks preview requires an initial presentation".to_string())
        }
    }
}

fn selected_provider_data(
    provider: &str,
    local: ProviderUsageSummary,
    live: LiveUsageSummary,
) -> PopoverPeekData {
    PopoverPeekData::Provider {
        summary: Box::new(ProviderUsageSummary {
            providers: local
                .providers
                .into_iter()
                .filter(|candidate| candidate.provider == provider)
                .collect(),
            totals: local.totals,
            agents: local.agents,
            generated_at: local.generated_at,
        }),
        live: Box::new(LiveUsageSummary {
            providers: live
                .providers
                .into_iter()
                .filter(|candidate| candidate.provider == provider)
                .collect(),
            errors: live
                .errors
                .into_iter()
                .filter(|error| error.provider == provider)
                .collect(),
            meters: live
                .meters
                .into_iter()
                .filter(|meter| meter.provider == provider)
                .collect(),
            generated_at: live.generated_at,
        }),
    }
}

#[tauri::command]
pub fn popover_peek_ready(
    window: tauri::WebviewWindow,
    generation: u64,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<bool, String> {
    validate_peek_caller(window.label())?;
    manager
        .renderer_ready(window.app_handle(), generation)
        .map_err(|error| error.to_string())
}

fn current_target(
    manager: &PopoverPeekManager,
    generation: u64,
) -> Result<PopoverPeekTarget, String> {
    let state = manager.state();
    if state.generation != generation {
        return Err("stale popover peek request".to_string());
    }
    state
        .target
        .ok_or_else(|| "the popover peek has no current target".to_string())
}

#[tauri::command]
pub fn popover_peek_presented(
    window: tauri::WebviewWindow,
    generation: u64,
    content_height: Option<f64>,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<bool, String> {
    validate_peek_caller(window.label())?;
    current_target(&manager, generation)?;
    manager
        .presented(window.app_handle(), generation, content_height)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn popover_peek_retarget_ready(
    window: tauri::WebviewWindow,
    generation: u64,
    content_height: Option<f64>,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<bool, String> {
    validate_peek_caller(window.label())?;
    manager
        .retarget_committed_with_height(window.app_handle(), generation, content_height)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn popover_peek_concealed(
    window: tauri::WebviewWindow,
    generation: u64,
    manager: tauri::State<'_, PopoverPeekManager>,
) -> Result<bool, String> {
    validate_peek_caller(window.label())?;
    Ok(manager.concealed(window.app_handle(), generation))
}

#[cfg(target_os = "macos")]
fn native_request(
    app: &tauri::AppHandle,
    command: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Generation {
        generation: u64,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Presentation {
        generation: u64,
        content_height: Option<f64>,
    }
    let manager = app.state::<PopoverPeekManager>();
    let result = match command {
        "get_popover_peek_state" => {
            if args != serde_json::json!({}) {
                return Err("unexpected state arguments".into());
            }
            serde_json::to_value(manager.state())
        }
        "get_popover_peek_data" => {
            let args: Generation =
                serde_json::from_value(args).map_err(|error| error.to_string())?;
            serde_json::to_value(peek_data(app, &manager, args.generation)?)
        }
        "popover_peek_ready" | "popover_peek_concealed" => {
            let args: Generation =
                serde_json::from_value(args).map_err(|error| error.to_string())?;
            let accepted = if command == "popover_peek_ready" {
                manager
                    .renderer_ready(app, args.generation)
                    .map_err(|error| error.to_string())?
            } else {
                manager.concealed(app, args.generation)
            };
            serde_json::to_value(accepted)
        }
        "popover_peek_presented" | "popover_peek_retarget_ready" => {
            let args: Presentation =
                serde_json::from_value(args).map_err(|error| error.to_string())?;
            let accepted = if command == "popover_peek_presented" {
                current_target(&manager, args.generation)?;
                manager.presented(app, args.generation, args.content_height)
            } else {
                manager.retarget_committed_with_height(app, args.generation, args.content_height)
            };
            serde_json::to_value(accepted.map_err(|error| error.to_string())?)
        }
        "note_interaction" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Note {
                interaction: crate::analytics::event::Interaction,
            }
            let note: Note = serde_json::from_value(args).map_err(|error| error.to_string())?;
            validate_native_interaction(&note.interaction)?;
            if manager.state().visible {
                crate::analytics::record_interaction(app, note.interaction);
            }
            Ok(serde_json::Value::Null)
        }
        _ => return Err("unsupported native preview command".into()),
    };
    result.map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn validate_native_interaction(
    interaction: &crate::analytics::event::Interaction,
) -> Result<(), String> {
    use crate::analytics::event::{Interaction, StateSurface, Surface};
    match interaction {
        Interaction::SurfaceViewed {
            surface: Surface::ProviderPreview | Surface::ChecksPreview,
            ..
        }
        | Interaction::SurfaceStateObserved {
            surface: StateSurface::ProviderPreview | StateSurface::ChecksPreview,
            ..
        }
        | Interaction::LiveUsageStateObserved { .. } => Ok(()),
        _ => Err("the native preview cannot report this interaction".into()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn rebuild_if_targeted(app: &tauri::AppHandle) {
    if let Some(manager) = app.try_state::<PopoverPeekManager>()
        && let Err(error) = manager.rebuild_if_targeted(app)
    {
        ::tracing::warn!(event = "popover_peek_rebuild_failed", companion_label = LABEL, error = %error);
    }
}

pub fn conceal_now(app: &tauri::AppHandle) {
    if let Some(manager) = app.try_state::<PopoverPeekManager>()
        && let Err(error) = manager.conceal_for_anchor_hide(app)
    {
        ::tracing::warn!(event = "popover_peek_conceal_failed", companion_label = LABEL, error = %error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{
        LiveProviderUsage, LiveUsageFreshness, LiveUsageMeter, LiveUsageSourceError,
        LiveUsageSupport, ProviderUsage, ProviderUsageStaleness, ProviderUsageState,
        ProviderUsageWindows,
    };

    fn local_provider(provider: &str) -> ProviderUsage {
        ProviderUsage {
            provider: provider.to_string(),
            display_name: provider.to_string(),
            account_key: None,
            agents: Vec::new(),
            state: ProviderUsageState::Estimated,
            staleness: ProviderUsageStaleness::Fresh,
            windows: ProviderUsageWindows::default(),
            last_activity_at: None,
        }
    }

    fn live_provider(provider: &str) -> LiveProviderUsage {
        LiveProviderUsage {
            provider: provider.to_string(),
            display_name: provider.to_string(),
            account_key: None,
            support: LiveUsageSupport::Live,
            freshness: LiveUsageFreshness::Fresh,
            source_label: "test".to_string(),
            observed_at: "now".to_string(),
            windows: Vec::new(),
            extra_usage: None,
            reset_credits: None,
            plan: None,
            account_uuid: None,
            account_email: None,
        }
    }

    fn live_error(provider: &str) -> LiveUsageSourceError {
        LiveUsageSourceError {
            source: provider.to_string(),
            provider: provider.to_string(),
            display_name: provider.to_string(),
            category: "unavailable".to_string(),
        }
    }

    #[test]
    fn caller_labels_are_restricted_by_command_direction() {
        assert!(validate_popover_caller("popover").is_ok());
        assert!(validate_popover_caller("settings").is_err());
        assert!(validate_peek_caller(LABEL).is_ok());
        assert!(validate_peek_caller("popover").is_err());
        assert!(validate_peek_caller("settings").is_err());
        assert!(validate_state_caller(LABEL).is_ok());
        assert!(validate_state_caller("popover").is_ok());
        assert!(validate_state_caller("settings").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_analytics_cannot_report_unrelated_surfaces() {
        use crate::analytics::event::{Interaction, Origin, Surface};
        assert!(
            validate_native_interaction(&Interaction::SurfaceViewed {
                surface: Surface::ProviderPreview,
                origin: Origin::User
            })
            .is_ok()
        );
        assert!(
            validate_native_interaction(&Interaction::SurfaceViewed {
                surface: Surface::Settings,
                origin: Origin::User
            })
            .is_err()
        );
    }

    #[test]
    fn checks_target_and_data_have_bounded_shapes() {
        assert_eq!(
            serde_json::to_value(PopoverPeekTarget::Checks).unwrap(),
            serde_json::json!({ "kind": "checks" })
        );
        let value = serde_json::json!({
            "kind": "checks",
            "presentation": {
                "failures": [],
                "wins": [],
                "unavailable": [],
                "refreshUnavailable": false,
                "estimate": {
                    "tokenBurnBasisPoints": 1000
                }
            }
        });
        let data: PopoverPeekData = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(data).unwrap(), value);
    }

    #[test]
    fn checks_data_rejects_unbounded_or_unknown_content() {
        let unknown = serde_json::json!({
            "kind": "checks",
            "presentation": {
                "failures": [],
                "wins": [],
                "unavailable": [],
                "refreshUnavailable": false,
                "estimate": {
                    "tokenBurnBasisPoints": 1000
                },
                "transcript": "must not cross this boundary"
            }
        });
        assert!(serde_json::from_value::<PopoverPeekData>(unknown).is_err());

        let invalid = ChecksPresentation {
            failures: Vec::new(),
            wins: Vec::new(),
            unavailable: Vec::new(),
            refresh_unavailable: false,
            estimate: ChecksEstimate {
                token_burn_basis_points: Some(10_001),
            },
        };
        assert!(validate_checks_presentation(&invalid).is_err());

        let duplicate = ChecksCategory {
            id: "cacheChurn".to_string(),
            finding: 1,
            clean: 0,
            unavailable: 0,
            estimated_token_burn_basis_points: Some(2_500),
        };
        let valid = ChecksPresentation {
            failures: vec![duplicate.clone()],
            wins: Vec::new(),
            unavailable: Vec::new(),
            refresh_unavailable: false,
            estimate: ChecksEstimate {
                token_burn_basis_points: Some(1_000),
            },
        };
        assert!(validate_checks_presentation(&valid).is_ok());

        let valid = ChecksPresentation {
            failures: Vec::new(),
            wins: vec![ChecksCategory {
                id: "sessionsOverDepth".to_string(),
                finding: 0,
                clean: 1,
                unavailable: 0,
                estimated_token_burn_basis_points: Some(0),
            }],
            unavailable: Vec::new(),
            refresh_unavailable: false,
            estimate: ChecksEstimate {
                token_burn_basis_points: Some(0),
            },
        };
        assert!(validate_checks_presentation(&valid).is_ok());

        let invalid = ChecksPresentation {
            failures: vec![duplicate.clone(), duplicate],
            wins: Vec::new(),
            unavailable: Vec::new(),
            refresh_unavailable: false,
            estimate: ChecksEstimate {
                token_burn_basis_points: Some(1_000),
            },
        };
        assert!(validate_checks_presentation(&invalid).is_err());

        let invalid = ChecksPresentation {
            failures: vec![ChecksCategory {
                id: "cacheChurn".to_string(),
                finding: 1,
                clean: 0,
                unavailable: 0,
                estimated_token_burn_basis_points: Some(10_001),
            }],
            wins: Vec::new(),
            unavailable: Vec::new(),
            refresh_unavailable: false,
            estimate: ChecksEstimate {
                token_burn_basis_points: Some(1_000),
            },
        };
        assert!(validate_checks_presentation(&invalid).is_err());
    }

    #[test]
    fn provider_data_contains_only_the_selected_provider() {
        let data = selected_provider_data(
            "openai",
            ProviderUsageSummary {
                providers: vec![local_provider("anthropic"), local_provider("openai")],
                totals: ProviderUsageWindows::default(),
                agents: Vec::new(),
                generated_at: "now".to_string(),
            },
            LiveUsageSummary {
                providers: vec![live_provider("anthropic"), live_provider("openai")],
                errors: vec![live_error("anthropic"), live_error("openai")],
                meters: vec![
                    LiveUsageMeter {
                        provider: "anthropic".to_string(),
                        display_name: "Claude".to_string(),
                        shown: true,
                    },
                    LiveUsageMeter {
                        provider: "openai".to_string(),
                        display_name: "Codex".to_string(),
                        shown: true,
                    },
                ],
                generated_at: "now".to_string(),
            },
        );

        let PopoverPeekData::Provider { summary, live } = data else {
            panic!("expected provider data");
        };
        assert_eq!(summary.providers.len(), 1);
        assert_eq!(summary.providers[0].provider, "openai");
        assert_eq!(live.providers.len(), 1);
        assert_eq!(live.providers[0].provider, "openai");
        assert_eq!(live.errors.len(), 1);
        assert_eq!(live.errors[0].provider, "openai");
        assert_eq!(live.meters.len(), 1);
        assert_eq!(live.meters[0].provider, "openai");
    }
}
