//! Local provider usage, session-limit allocations, and live usage commands.
//!
//! "Local" figures come from sessions already indexed on this machine; "live"
//! figures come from asking each provider directly. Both surfaces render side
//! by side in the popover, so their commands share a module.

use super::*;

/// Per-provider token and cost totals derived from the sessions already on this
/// machine.
///
/// `utc_offset_minutes` is the webview's own offset from UTC. The shell asks
/// for it rather than reading the platform's, because "today" and "this month"
/// are the reader's calendar days, and resolving the local offset inside a
/// multi-threaded process is not reliable on every platform. Omitting it falls
/// back to UTC, which is right for a machine running on it and off by at most
/// one day's boundary for anyone else.
///
/// Nothing here contacts a provider. Every figure comes from
/// [`crate::provider_usage`], which reads the local database and the engine's
/// active runtime pricing snapshot.
#[tauri::command]
pub async fn get_provider_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<ProviderUsageSummary> {
    run_blocking(move || provider_usage_summary(&app, utc_offset_minutes)).await
}

pub(crate) fn provider_usage_summary(
    app: &tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<ProviderUsageSummary> {
    let now = scan::unix_now();
    let offset = utc_offset_minutes.unwrap_or(0);
    let since = provider_usage::lookback_start(now, offset);
    let evidence = app.state::<Store>().usage_evidence(since).map_err(fail)?;
    let summary = provider_usage::summarize(&evidence, now, offset);
    ::tracing::debug!(
        event = "provider_attribution_summary",
        sessions = evidence.len(),
        groups = summary.providers.len(),
        assigned_groups = summary
            .providers
            .iter()
            .filter(|provider| provider.account_key.is_some())
            .count(),
        unassigned_groups = summary
            .providers
            .iter()
            .filter(
                |provider| provider.provider != provider_usage::providers::UNKNOWN
                    && provider.account_key.is_none()
            )
            .count(),
        unattributed_groups = summary
            .providers
            .iter()
            .filter(|provider| provider.provider == provider_usage::providers::UNKNOWN)
            .count(),
        detected_groups = summary
            .providers
            .iter()
            .filter(|provider| provider.state == crate::dto::ProviderUsageState::Detected)
            .count(),
    );
    Ok(summary)
}

/// Estimate each recent session's share of its provider account's learned
/// dollars-per-percent limit factor.
#[tauri::command]
pub async fn get_session_limit_allocations(
    app: tauri::AppHandle,
) -> CommandResult<SessionLimitAllocationSummary> {
    let now = scan::unix_now();
    let store = app.state::<Store>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let settings = store.settings().map_err(fail)?;
        let since = now.saturating_sub(i64::from(settings.activity_window_days) * 86_400);
        let sessions = store
            .recent_sessions_excluding(since, MAX_ACTIVITY_ROWS, &settings.disabled_agents)
            .map_err(fail)?;
        let allocations = session_limit_allocations(&store, &sessions).map_err(fail)?;
        Ok(SessionLimitAllocationSummary {
            allocations,
            generated_at: crate::store::iso_from_epoch(Some(now)),
        })
    })
    .await
    .map_err(fail)?
}

/// Per-provider token maps ready for [`price_breakdown`], keyed the same way
/// `pricing_breakdown_json` keys its entries, so a fast-mode turn prices at
/// its fast rate instead of the base rate the factor was not learned at.
///
/// `pricing_breakdown_json` keys are `turn_pricing_key(model, speed)`
/// (`crates/antiburn-local/src/analysis/pricing.rs`): the model as
/// `model_breakdown_json` names it, with `-fast` appended when the turn ran
/// fast and the model's own name does not already end that way. A pricing key
/// belongs to a provider when it names one of that provider's attributed
/// models directly, or with a trailing `-fast` removed.
///
/// Falls back to pricing `model_breakdown_json`'s own attribution directly
/// when `pricing_breakdown_json` is empty or does not parse, since that is
/// the only breakdown available then.
pub(crate) fn provider_priced_models(
    attributed: &BTreeMap<&'static str, provider_usage::Attributed>,
    pricing_breakdown_json: &str,
) -> HashMap<&'static str, HashMap<String, ModelTokens>> {
    let pricing: BTreeMap<String, ModelTokens> =
        serde_json::from_str(pricing_breakdown_json).unwrap_or_default();
    if pricing.is_empty() {
        return attributed
            .iter()
            .map(|(&provider, attributed)| {
                let priced: HashMap<String, ModelTokens> = attributed
                    .models
                    .iter()
                    .map(|(model, tokens)| (model.clone(), tokens.clone()))
                    .collect();
                (provider, priced)
            })
            .collect();
    }
    let mut provider_for_model: HashMap<&str, &'static str> = HashMap::new();
    for (&provider, attributed) in attributed {
        for model in attributed.models.keys() {
            provider_for_model.insert(model.as_str(), provider);
        }
    }
    let mut by_provider: HashMap<&'static str, HashMap<String, ModelTokens>> = HashMap::new();
    for (key, tokens) in &pricing {
        let provider = provider_for_model.get(key.as_str()).copied().or_else(|| {
            key.strip_suffix("-fast")
                .and_then(|base| provider_for_model.get(base).copied())
        });
        if let Some(provider) = provider {
            by_provider
                .entry(provider)
                .or_default()
                .insert(key.clone(), tokens.clone());
        }
    }
    by_provider
}

/// One row per session, provider, and lane: the session's inclusive dollars
/// divided by the factor point in effect at its last activity.
///
/// A session with no resolved account for a provider its usage attributes
/// to, or a lane with no factor point yet, contributes no row for that
/// provider or lane. A session that spends under more than one provider
/// (a bring-your-own agent that switched models) contributes one row per
/// provider.
pub(crate) fn session_limit_allocations(
    store: &Store,
    sessions: &[SessionRecord],
) -> anyhow::Result<Vec<SessionLimitAllocation>> {
    let keys: Vec<SessionKey> = sessions.iter().map(|session| session.key.clone()).collect();
    let bound = store.session_bound_accounts(&keys)?;
    let analyses = store.analyses(&keys)?;
    let mut known_accounts: HashMap<&'static str, HashMap<String, BTreeSet<String>>> =
        HashMap::new();

    let mut allocations = Vec::new();
    for session in sessions {
        let Some(updated_at_epoch) = session.updated_at_epoch else {
            continue;
        };
        let Some(analysis) = analyses.get(&session.key) else {
            continue;
        };
        let models: BTreeMap<String, ModelTokens> =
            serde_json::from_str(&analysis.model_breakdown_json).unwrap_or_default();
        if models.is_empty() {
            continue;
        }
        let hints: Vec<ProviderHint> = analysis
            .provider_hints_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let attributed = provider_usage::attribute(&session.key.agent, models, &hints);
        let priced_by_provider =
            provider_priced_models(&attributed, &analysis.pricing_breakdown_json);
        for &provider in attributed.keys() {
            let Some(priced) = priced_by_provider.get(provider) else {
                continue;
            };
            let Some(cost) = price_breakdown(priced) else {
                continue;
            };
            if !(cost.total_usd.is_finite() && cost.total_usd > 0.0) {
                continue;
            }
            let known = known_accounts
                .entry(provider)
                .or_insert_with(|| store.provider_known_accounts(provider).unwrap_or_default());
            let bound_for = bound.get(&(session.key.clone(), provider.to_string()));
            let Some(account_key) = crate::store::provider_limit::resolve_bound_account(
                bound_for,
                known.get(&session.key.agent),
            ) else {
                continue;
            };
            for (lane, metric) in [
                (
                    crate::store::provider_limit::LANE_WEEKLY,
                    crate::dto::SessionLimitMetric::Weekly,
                ),
                (
                    crate::store::provider_limit::LANE_FIVE_HOUR,
                    crate::dto::SessionLimitMetric::FiveHour,
                ),
            ] {
                let Ok(Some(point)) =
                    store.factor_point_at(provider, &account_key, lane, updated_at_epoch)
                else {
                    continue;
                };
                if !(point.usd_per_percent.is_finite() && point.usd_per_percent > 0.0) {
                    continue;
                }
                allocations.push(SessionLimitAllocation {
                    agent: session.key.agent.clone(),
                    session_id: session.key.session_id.clone(),
                    wsl_distro: session.wsl_distro.clone(),
                    metric,
                    provider: provider.to_string(),
                    display_name: provider_usage::providers::display_name(provider).to_string(),
                    account_key: Some(account_key.clone()),
                    window_id: lane.to_string(),
                    percent: cost.total_usd / point.usd_per_percent,
                    confidence: if point.method == "delta" {
                        "learned"
                    } else {
                        "seeded"
                    }
                    .to_string(),
                });
            }
        }
    }
    Ok(allocations)
}

/// How fresh a reading the refresh command asks each source's cooldown for.
///
/// This command is called from the popover, which polls it on its own 60 s
/// visible interval for as long as the popover stays visible (R6,
/// `USAGE_VISIBLE_POLL_MS` in `PopoverSession.ts`). Fifty seconds sits just
/// under that polling interval, so an open popover's own ordinary polling is
/// what keeps the reading current: every visible tick is close enough to the
/// cooldown's edge to trigger a real fetch, without this command itself
/// running a timer or a background task. The aggressive freshness is bounded
/// by someone actually looking — once the popover closes, nothing here keeps
/// polling on its behalf, and the background monitor's own, much longer,
/// `max_age` takes back over (see `usage_alerts::BACKGROUND_MAX_AGE`).
const POPOVER_LIVE_USAGE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(50);

/// Return cached limits and update inactive provider detection on a blocking thread.
///
/// This remains separate from [`get_provider_usage`]. That payload carries no
/// percentage, allowance, or reset anywhere, and a test proves it by
/// serializing the whole thing. Keeping the two apart means a limit surface
/// can exist without weakening the estimate surface's contract, and the views
/// layer them.
///
/// The live-usage setting gates the cached value too. Turning the feature off
/// removes its figures immediately without waiting for another refresh.
#[tauri::command]
pub async fn get_live_usage(
    app: tauri::AppHandle,
    _utc_offset_minutes: Option<i32>,
) -> CommandResult<LiveUsageSummary> {
    run_blocking(move || {
        // With live usage off no collection pass runs, so this is the one
        // place detection advances for the roster. Metadata-only here: the
        // reader has not opted in.
        let active = app
            .try_state::<Store>()
            .and_then(|store| store.settings().ok())
            .is_some_and(|settings| settings.live_usage_active());
        if !active && let Some(live) = app.try_state::<crate::usage_alerts::LiveUsage>() {
            let detection = provider_usage::live::detect_all(&live.sources, false);
            live.store_detection(detection);
        }
        Ok(cached_live_usage(&app))
    })
    .await
}

/// Keep this reader cache-only because synchronous popover IPC calls it.
/// Never read provider metadata or start subprocesses here.
pub(crate) fn cached_live_usage(app: &tauri::AppHandle) -> LiveUsageSummary {
    let summary = collected_live_usage(app);
    #[cfg(debug_assertions)]
    let summary = crate::tray::simulate_codex_only(app, summary);
    summary
}

fn collected_live_usage(app: &tauri::AppHandle) -> LiveUsageSummary {
    let settings = app
        .try_state::<Store>()
        .and_then(|store| store.settings().ok());
    let active = settings
        .as_ref()
        .is_some_and(|settings| settings.live_usage_active());
    let live = app.try_state::<crate::usage_alerts::LiveUsage>();
    if !active {
        // No readings, but keep the roster: Settings shows one switch for each
        // provider antiburn can meter, and the master switch does not remove
        // them. A roster is a list of capabilities, not a reading.
        let hidden = settings
            .map(|settings| settings.live_usage_hidden_providers)
            .unwrap_or_default();
        return LiveUsageSummary {
            meters: live
                .map(|live| {
                    provider_usage::live::roster(&live.sources, &hidden, &live.detection_snapshot())
                })
                .unwrap_or_default(),
            ..LiveUsageSummary::default()
        };
    }
    live.map(|live| live.snapshot()).unwrap_or_default()
}

/// Refresh the provider's own limit figures and publish the new snapshot.
///
/// An empty summary is the ordinary answer: no source has anything to say.
/// Sources that fail report separately, so absence and failure stay distinct.
///
/// `utc_offset_minutes` travels for one reason only: "used today" is a claim
/// about the reader's calendar day. The windows themselves are the provider's
/// own boundaries, stated as absolute instants, and owe nothing to it.
///
/// `async`, and every byte of the work handed to a blocking thread, for one
/// reason: a synchronous `#[tauri::command]` is run inline on the thread that
/// delivered the IPC message, and the summary this returns reaches a provider
/// over the network with `reqwest::blocking` — see
/// `provider_usage::live::sources::http::client`. A provider that accepts a
/// connection and then says nothing would hold that thread for the full
/// fifteen-second timeout, once per source, and the popover polls this about
/// once a minute while it is open. The reader would watch the whole app —
/// tray, popover, every window — stop answering. Nothing here needs the main
/// thread, so nothing here stays on it.
#[tauri::command]
pub async fn refresh_live_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<LiveUsageSummary> {
    // The sources deliberately expose a synchronous interface and include
    // blocking HTTP, Keychain, and subprocess work. The blocking pool is the
    // boundary for all of it.
    let utc_offset_minutes = utc_offset_minutes.unwrap_or(0);
    tauri::async_runtime::spawn_blocking(move || {
        crate::usage_alerts::refresh_publish_and_evaluate(
            &app,
            POPOVER_LIVE_USAGE_MAX_AGE,
            Some(utc_offset_minutes),
        )
    })
    .await
    .map_err(fail)
}
