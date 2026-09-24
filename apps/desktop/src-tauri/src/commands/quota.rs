//! Quota period, meter, and contribution queries.
//!
//! These commands read the periods a quota lane ran through, its meter
//! readings, and the sessions estimated to have contributed to each period's
//! spend. See `crate::provider_usage::quota` for the boundary resolver and
//! `crate::store::provider_limit` for the underlying store queries.

use super::*;
use crate::dto::{
    LiveProviderPlan, QuotaAccountPayload, QuotaAccountsPayload, QuotaBucketTotalPayload,
    QuotaContributionPayload, QuotaCurrentPeriodPayload, QuotaLanePayload, QuotaPeriodPayload,
    QuotaSamplePayload, QuotaSessionTotalPayload, QuotaUnattributedPayload, QuotaUsagePayload,
    QuotaUsageRequest, SessionQuotaEntryPayload, SessionQuotaPayload, SessionQuotaPeriodPayload,
    SessionQuotaRequest,
};
use crate::provider_usage::quota::share::{ShareInput, share_period_capped};

/// A quota query's range may not exceed this many days: enough for ten
/// weekly windows, bounded so one request cannot force an unbounded
/// turn-row scan.
const MAX_QUOTA_RANGE_DAYS: i64 = 70;

/// Every provider account this app has observed at least one quota period
/// for, with the lanes each one carries. Feeds the quota screen's account
/// and lane pickers.
#[tauri::command]
pub async fn get_quota_accounts(app: tauri::AppHandle) -> CommandResult<QuotaAccountsPayload> {
    run_blocking(move || {
        let now = scan::unix_now();
        let store = app.state::<Store>();
        let accounts = store.quota_accounts(now).map_err(fail)?;
        Ok(QuotaAccountsPayload {
            accounts: accounts.into_iter().map(quota_account_payload).collect(),
            generated_at: iso_from_epoch(Some(now)),
        })
    })
    .await
}

fn quota_account_payload(
    account: crate::store::provider_limit::QuotaAccount,
) -> QuotaAccountPayload {
    QuotaAccountPayload {
        provider: account.provider,
        display_name: account.display_name,
        account_key: account.account_key,
        lanes: account
            .lanes
            .into_iter()
            .map(|lane| QuotaLanePayload {
                lane: lane.lane,
                label: lane.label,
                current_period: lane
                    .current_period
                    .map(
                        |(starts_at_epoch, resets_at_epoch)| QuotaCurrentPeriodPayload {
                            starts_at_epoch,
                            resets_at_epoch,
                        },
                    ),
                first_observed_epoch: lane.first_observed_epoch,
            })
            .collect(),
    }
}

fn boundary_source_str(source: crate::provider_usage::quota::BoundarySource) -> &'static str {
    use crate::provider_usage::quota::BoundarySource;
    match source {
        BoundarySource::Reported => "reported",
        BoundarySource::Derived => "derived",
        BoundarySource::Cadence => "cadence",
        BoundarySource::TurnGap => "turnGap",
        BoundarySource::Truncated => "truncated",
    }
}

/// `"learned"` from a meter delta, `"seeded"` from a single first-reading
/// estimate. Mirrors the same mapping [`session_limit_allocations`] uses.
fn factor_confidence(point: &crate::store::provider_limit::FactorPoint) -> &'static str {
    if point.method == "delta" {
        "learned"
    } else {
        "seeded"
    }
}

/// The reader-facing name for a lane when no observed period is on hand to
/// read a model-scoped label from: the two fixed lanes' own names, or the
/// raw lane id as a last resort.
fn lane_label_fallback(lane: &str) -> String {
    match lane {
        crate::store::provider_limit::LANE_WEEKLY => "Weekly".to_string(),
        crate::store::provider_limit::LANE_FIVE_HOUR => "5-hour".to_string(),
        other => other.to_string(),
    }
}

/// A session's title and WSL distro, or `None` for either when the session
/// record does not carry one.
type SessionTitleAndDistro = (Option<String>, Option<String>);

/// One session's title and WSL distro, batched for every session a quota
/// query's contributions touch.
fn session_titles_and_distros(
    store: &Store,
    keys: &HashSet<SessionKey>,
) -> CommandResult<HashMap<SessionKey, SessionTitleAndDistro>> {
    let keys: Vec<SessionKey> = keys.iter().cloned().collect();
    let records = store
        .session_records_for_session_keys(&keys)
        .map_err(fail)?;
    Ok(records
        .into_iter()
        .map(|record| (record.key.clone(), (record.title, record.wsl_distro)))
        .collect())
}

/// Every quota window a lane ran through a range, its meter readings, and
/// the sessions estimated to have contributed to it.
#[tauri::command]
pub async fn get_quota_usage(
    app: tauri::AppHandle,
    request: QuotaUsageRequest,
) -> CommandResult<QuotaUsagePayload> {
    validate_quota_range(request.range_start_epoch, request.range_end_epoch)?;
    run_blocking(move || quota_usage(&app, request)).await
}

/// A quota range must end after its start and span at most
/// [`MAX_QUOTA_RANGE_DAYS`].
fn validate_quota_range(range_start_epoch: i64, range_end_epoch: i64) -> CommandResult<()> {
    if range_end_epoch <= range_start_epoch {
        return Err("quota range end must be after its start".to_string());
    }
    if range_end_epoch - range_start_epoch > MAX_QUOTA_RANGE_DAYS * 86_400 {
        return Err(format!(
            "quota range may not exceed {MAX_QUOTA_RANGE_DAYS} days"
        ));
    }
    Ok(())
}

fn quota_usage(
    app: &tauri::AppHandle,
    request: QuotaUsageRequest,
) -> CommandResult<QuotaUsagePayload> {
    let now = scan::unix_now();
    let store = app.state::<Store>();
    quota_usage_for_store(&store, now, request)
}

/// [`quota_usage`]'s body, over a borrowed [`Store`] rather than an
/// [`tauri::AppHandle`], so it can run against an in-memory store in a test
/// without standing up a Tauri app.
pub(super) fn quota_usage_for_store(
    store: &Store,
    now: i64,
    request: QuotaUsageRequest,
) -> CommandResult<QuotaUsagePayload> {
    let input = store
        .quota_turn_input(request.range_start_epoch, request.range_end_epoch)
        .map_err(fail)?
        .ok_or_else(|| "too much turn activity in this range".to_string())?;
    let model_scope = request
        .lane
        .strip_prefix(crate::store::provider_limit::MODEL_LANE_PREFIX);
    let dollars = input.for_account(&request.provider, &request.account_key, model_scope);
    // A single-account command has no shared scan to reuse, so it keeps
    // computing its own five-hour turn epochs, the way it always has.
    quota_usage_with_turn_dollars(store, now, request, &dollars, None)
}

pub(super) fn quota_usage_with_turn_dollars(
    store: &Store,
    now: i64,
    request: QuotaUsageRequest,
    dollars: &crate::store::provider_limit::AccountTurnDollars,
    turn_minutes: Option<&crate::store::provider_limit::TurnMinutes>,
) -> CommandResult<QuotaUsagePayload> {
    let QuotaUsageRequest {
        provider,
        account_key,
        lane,
        range_start_epoch,
        range_end_epoch,
    } = request;
    let lane_duration = crate::store::provider_limit::lane_duration_seconds(&lane);
    let model_scope_owned = lane
        .strip_prefix(crate::store::provider_limit::MODEL_LANE_PREFIX)
        .map(str::to_string);
    let model_scope = model_scope_owned.as_deref();

    let observed = store
        .quota_periods_for_lane(
            &provider,
            &account_key,
            &lane,
            range_start_epoch,
            range_end_epoch,
        )
        .map_err(fail)?;
    let turn_epochs = if lane == crate::store::provider_limit::LANE_FIVE_HOUR {
        match turn_minutes {
            Some(turn_minutes) => turn_minutes.for_account(&provider, &account_key),
            None => store
                .attributed_turn_epochs(&provider, &account_key, range_start_epoch, range_end_epoch)
                .map_err(fail)?,
        }
    } else {
        Vec::new()
    };
    let lane_label = observed
        .first()
        .map(|period| crate::store::provider_limit::lane_label(&lane, period))
        .unwrap_or_else(|| lane_label_fallback(&lane));
    let mut periods = crate::provider_usage::quota::resolve_periods(
        &lane,
        lane_duration,
        &observed,
        &turn_epochs,
        range_start_epoch,
        range_end_epoch,
        now,
    );
    periods.sort_by_key(|period| period.starts_at_epoch);

    let points = store
        .factor_points_for_lane(&provider, &account_key, &lane)
        .map_err(fail)?;

    let bucketed = dollars.by_bucket(model_scope);

    let by_period: Vec<Vec<_>> = periods
        .iter()
        .map(|period| {
            bucketed
                .iter()
                .filter_map(|row| {
                    row.in_period(period.starts_at_epoch, period.resets_at_epoch, now)
                })
                .collect()
        })
        .collect();

    let session_keys: HashSet<SessionKey> = bucketed.iter().map(|row| row.key.clone()).collect();
    let session_titles = session_titles_and_distros(store, &session_keys)?;

    let period_ids: Vec<i64> = periods
        .iter()
        .filter_map(|period| period.period_id)
        .collect();
    let samples_by_period = store.quota_period_samples_for(&period_ids).map_err(fail)?;

    let mut period_payloads = Vec::with_capacity(periods.len());
    for (period, rows) in periods.iter().zip(by_period) {
        let samples: Vec<QuotaSamplePayload> = period
            .period_id
            .and_then(|period_id| samples_by_period.get(&period_id))
            .into_iter()
            .flatten()
            .map(|observation| QuotaSamplePayload {
                observed_at_epoch: observation.observed_at_epoch,
                used_percent: observation.used_percent,
                fresh: observation.is_fresh,
                authoritative: observation.is_authoritative,
            })
            .collect();

        // The period's authoritative readings, inside its own bounds and
        // sorted ascending: `share_period`'s own contract. Truncation can
        // move a period's reset earlier than a reading tied to its
        // `period_id`, so this filters by the period's current bounds
        // rather than trusting every stored observation.
        let readings: Vec<(i64, f64)> = samples
            .iter()
            .filter(|sample| sample.authoritative)
            .filter_map(|sample| {
                sample
                    .used_percent
                    .map(|percent| (sample.observed_at_epoch, percent))
            })
            .filter(|&(observed_at_epoch, _)| {
                observed_at_epoch >= period.starts_at_epoch
                    && observed_at_epoch < period.resets_at_epoch
            })
            .collect();
        let bucket_dollars: Vec<&[(i64, f64)]> =
            rows.iter().map(|row| row.usage.as_slice()).collect();
        let shared = share_period_capped(
            &ShareInput {
                start: period.starts_at_epoch,
                reset: period.resets_at_epoch,
                readings: &readings,
                buckets: &bucket_dollars,
                points: &points,
            },
            now,
        );

        let mut contributions = Vec::new();
        let mut per_session_bound: HashMap<SessionKey, (f64, f64, bool)> = HashMap::new();
        let mut unattributed_usd = 0.0;
        let mut unattributed_sessions: HashSet<SessionKey> = HashSet::new();
        let mut unattributed_by_bucket: BTreeMap<i64, (f64, f64, bool)> = BTreeMap::new();
        for (row, percent) in rows.iter().zip(shared.bucket_percent.iter().copied()) {
            match &row.account {
                crate::store::provider_limit::Resolved::Bound(_) => {
                    let wsl_distro = session_titles
                        .get(&row.key)
                        .and_then(|entry| entry.1.clone());
                    contributions.push(QuotaContributionPayload {
                        agent: row.key.agent.clone(),
                        session_id: row.key.session_id.clone(),
                        wsl_distro,
                        bucket_start_epoch: row.bucket_start_epoch,
                        usd: row.usd,
                        percent,
                    });
                    let entry = per_session_bound
                        .entry(row.key.clone())
                        .or_insert((0.0, 0.0, false));
                    entry.0 += row.usd;
                    if let Some(p) = percent {
                        entry.1 += p;
                        entry.2 = true;
                    }
                }
                crate::store::provider_limit::Resolved::Unbound => {
                    unattributed_usd += row.usd;
                    unattributed_sessions.insert(row.key.clone());
                    let entry = unattributed_by_bucket
                        .entry(row.bucket_start_epoch)
                        .or_insert((0.0, 0.0, false));
                    entry.0 += row.usd;
                    if let Some(p) = percent {
                        entry.1 += p;
                        entry.2 = true;
                    }
                }
            }
        }
        let unattributed_buckets: Vec<QuotaBucketTotalPayload> = unattributed_by_bucket
            .into_iter()
            .map(
                |(bucket_start_epoch, (usd, percent_sum, any_percent))| QuotaBucketTotalPayload {
                    bucket_start_epoch,
                    usd,
                    percent: any_percent.then_some(percent_sum),
                },
            )
            .collect();
        let mut sessions: Vec<QuotaSessionTotalPayload> = per_session_bound
            .into_iter()
            .map(|(key, (usd, percent_sum, any_percent))| {
                let (title, wsl_distro) = session_titles.get(&key).cloned().unwrap_or_default();
                QuotaSessionTotalPayload {
                    agent: key.agent.clone(),
                    session_id: key.session_id.clone(),
                    wsl_distro,
                    title,
                    usd,
                    percent: any_percent.then_some(percent_sum),
                }
            })
            .collect();
        sessions.sort_by(|left, right| right.usd.total_cmp(&left.usd));
        let unattributed_percent = unattributed_buckets
            .iter()
            .any(|bucket| bucket.percent.is_some())
            .then(|| {
                unattributed_buckets
                    .iter()
                    .filter_map(|bucket| bucket.percent)
                    .sum()
            });

        let unexplained_buckets: Vec<QuotaBucketTotalPayload> = shared
            .unexplained
            .iter()
            .map(|&(bucket_start_epoch, percent)| QuotaBucketTotalPayload {
                bucket_start_epoch,
                usd: 0.0,
                percent: Some(percent),
            })
            .collect();
        // `None` only when the period has no reading at all: with a
        // reading, every segment's rise is accounted somewhere, even a
        // fully unexplained one, so the sum is always defined.
        let unexplained_percent = shared.coverage_until.map(|_| {
            unexplained_buckets
                .iter()
                .filter_map(|bucket| bucket.percent)
                .sum()
        });

        let estimated_percent = {
            let any = sessions.iter().any(|session| session.percent.is_some())
                || unattributed_percent.is_some()
                || unexplained_percent.is_some();
            any.then(|| {
                sessions
                    .iter()
                    .filter_map(|session| session.percent)
                    .sum::<f64>()
                    + unattributed_percent.unwrap_or(0.0)
                    + unexplained_percent.unwrap_or(0.0)
            })
        };

        period_payloads.push(QuotaPeriodPayload {
            period_id: period.period_id,
            starts_at_epoch: period.starts_at_epoch,
            resets_at_epoch: period.resets_at_epoch,
            start_source: boundary_source_str(period.start_source).to_string(),
            reset_source: boundary_source_str(period.reset_source).to_string(),
            samples,
            contributions,
            sessions,
            unattributed: QuotaUnattributedPayload {
                usd: unattributed_usd,
                percent: unattributed_percent,
                session_count: unattributed_sessions.len() as u32,
            },
            unattributed_buckets,
            estimated_percent,
            unexplained_buckets,
            unexplained_percent,
        });
    }

    Ok(QuotaUsagePayload {
        provider,
        account_key,
        lane,
        lane_label,
        range_start_epoch,
        range_end_epoch,
        periods: period_payloads,
        generated_at: iso_from_epoch(Some(now)),
    })
}

/// The quota windows one session's turns fell in, across every provider and
/// lane its usage attributes to: one entry per `(provider, lane, period)`.
///
/// A session with no resolved account for a provider its usage attributes to
/// still gets one entry for that provider, carrying its priced total under
/// `confidence: "unbound"`, `percent: None`, and no period: without a
/// resolved account there is no lane-specific window to credit it to.
#[tauri::command]
pub async fn get_session_quota(
    app: tauri::AppHandle,
    request: SessionQuotaRequest,
) -> CommandResult<SessionQuotaPayload> {
    run_blocking(move || session_quota(&app, request)).await
}

fn session_quota(
    app: &tauri::AppHandle,
    request: SessionQuotaRequest,
) -> CommandResult<SessionQuotaPayload> {
    let now = scan::unix_now();
    let store = app.state::<Store>();
    session_quota_for_store(&store, now, request)
}

/// [`session_quota`]'s body, over a borrowed [`Store`] rather than an
/// [`tauri::AppHandle`], so it can run against an in-memory store in a test
/// without standing up a Tauri app.
fn session_quota_for_store(
    store: &Store,
    now: i64,
    request: SessionQuotaRequest,
) -> CommandResult<SessionQuotaPayload> {
    let key = SessionKey::for_session(
        &request.agent,
        &request.session_id,
        request.wsl_distro.as_deref(),
    );
    let empty = || SessionQuotaPayload {
        entries: Vec::new(),
        generated_at: iso_from_epoch(Some(now)),
    };
    let Some((min_epoch, max_epoch)) = store.session_turn_epoch_bounds(&key).map_err(fail)? else {
        return Ok(empty());
    };
    let Some(analysis) = store
        .analyses(std::slice::from_ref(&key))
        .map_err(fail)?
        .remove(&key)
    else {
        return Ok(empty());
    };
    let models: BTreeMap<String, ModelTokens> =
        serde_json::from_str(&analysis.model_breakdown_json).unwrap_or_default();
    if models.is_empty() {
        return Ok(empty());
    }
    let hints: Vec<ProviderHint> = analysis
        .provider_hints_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let attributed = provider_usage::attribute(&key.agent, models, &hints);
    let bound = store
        .session_bound_accounts(std::slice::from_ref(&key))
        .map_err(fail)?;

    let accounts = store.quota_accounts(now).map_err(fail)?;
    let mut entries = Vec::new();
    for &provider in attributed.keys() {
        let known = store.provider_known_accounts(provider).map_err(fail)?;
        let bound_for = bound.get(&(key.clone(), provider.to_string()));
        let display_name = provider_usage::providers::display_name(provider).to_string();

        let Some(account_key) =
            crate::store::provider_limit::resolve_bound_account(bound_for, known.get(&key.agent))
        else {
            let priced_by_provider =
                provider_priced_models(&attributed, &analysis.pricing_breakdown_json);
            let usd = priced_by_provider
                .get(provider)
                .and_then(price_breakdown)
                .map(|cost| cost.total_usd)
                .unwrap_or(0.0);
            if usd > 0.0 {
                entries.push(SessionQuotaEntryPayload {
                    provider: provider.to_string(),
                    display_name,
                    account_key: None,
                    lane: None,
                    lane_label: None,
                    period: None,
                    usd,
                    percent: None,
                    confidence: "unbound".to_string(),
                    plan: None,
                });
            }
            continue;
        };

        let Some(account) = accounts
            .iter()
            .find(|account| account.provider == provider && account.account_key == account_key)
        else {
            continue;
        };

        let plan = account_plan(store, provider, &account_key, &account.lanes)?;

        for lane in &account.lanes {
            let lane_duration = crate::store::provider_limit::lane_duration_seconds(&lane.lane);
            let range_end = max_epoch + 1;
            // Cap the range at MAX_QUOTA_RANGE_DAYS. The card shows at most
            // the last MAX_QUOTA_RANGE_DAYS days of a long-running session,
            // the same bound the Limits screen applies to one request.
            let range_start =
                (min_epoch - lane_duration).max(range_end - MAX_QUOTA_RANGE_DAYS * 86_400);
            let observed = store
                .quota_periods_for_lane(provider, &account_key, &lane.lane, range_start, range_end)
                .map_err(fail)?;
            let turn_epochs = if lane.lane == crate::store::provider_limit::LANE_FIVE_HOUR {
                store
                    .attributed_turn_epochs(provider, &account_key, range_start, range_end)
                    .map_err(fail)?
            } else {
                Vec::new()
            };
            let periods = crate::provider_usage::quota::resolve_periods(
                &lane.lane,
                lane_duration,
                &observed,
                &turn_epochs,
                range_start,
                range_end,
                now,
            );
            let model_scope = lane
                .lane
                .strip_prefix(crate::store::provider_limit::MODEL_LANE_PREFIX);
            let points = store
                .factor_points_for_lane(provider, &account_key, &lane.lane)
                .map_err(fail)?;
            for period in periods.iter().filter(|period| {
                period.starts_at_epoch < range_end && period.resets_at_epoch > min_epoch
            }) {
                let Some(dollars) = store
                    .attributed_turn_dollars_by_bucket(
                        provider,
                        &account_key,
                        model_scope,
                        period.starts_at_epoch,
                        period.resets_at_epoch,
                    )
                    .map_err(fail)?
                else {
                    continue;
                };
                let samples = match period.period_id {
                    Some(period_id) => store.quota_period_samples(period_id).map_err(fail)?,
                    None => Vec::new(),
                };
                let readings: Vec<(i64, f64)> = samples
                    .iter()
                    .filter(|observation| observation.is_authoritative)
                    .filter_map(|observation| {
                        observation
                            .used_percent
                            .map(|percent| (observation.observed_at_epoch, percent))
                    })
                    .filter(|&(observed_at_epoch, _)| {
                        observed_at_epoch >= period.starts_at_epoch
                            && observed_at_epoch < period.resets_at_epoch
                    })
                    .collect();
                let dollars: Vec<_> = dollars
                    .into_iter()
                    .filter_map(|row| {
                        row.in_period(period.starts_at_epoch, period.resets_at_epoch, now)
                    })
                    .collect();
                let bucket_dollars: Vec<&[(i64, f64)]> =
                    dollars.iter().map(|row| row.usage.as_slice()).collect();
                let shared = share_period_capped(
                    &ShareInput {
                        start: period.starts_at_epoch,
                        reset: period.resets_at_epoch,
                        readings: &readings,
                        buckets: &bucket_dollars,
                        points: &points,
                    },
                    now,
                );

                let mut usd = 0.0;
                let mut percent_sum = 0.0;
                let mut any_percent = false;
                let mut has_own_bucket = false;
                let mut all_shared = true;
                for (row, percent) in dollars.iter().zip(shared.bucket_percent.iter().copied()) {
                    if row.key != key {
                        continue;
                    }
                    has_own_bucket = true;
                    usd += row.usd;
                    if let Some(p) = percent {
                        percent_sum += p;
                        any_percent = true;
                    }
                    let in_shared_segment = shared.coverage_until.is_some_and(|coverage_until| {
                        row.usage
                            .iter()
                            .all(|&(ts_ms, _)| ts_ms <= coverage_until.saturating_mul(1_000))
                    });
                    if !in_shared_segment {
                        all_shared = false;
                    }
                }
                if !has_own_bucket || usd <= 0.0 {
                    continue;
                }

                // Keep the known contribution if the tail has no factor.
                if !any_percent {
                    continue;
                }
                let confidence = if all_shared {
                    "measured".to_string()
                } else {
                    match crate::provider_usage::quota::factor_point_at_or_earliest(
                        &points,
                        period.resets_at_epoch.min(now),
                    ) {
                        Some(point) => factor_confidence(point).to_string(),
                        None => "measured".to_string(),
                    }
                };
                let percent = Some(percent_sum);

                entries.push(SessionQuotaEntryPayload {
                    provider: provider.to_string(),
                    display_name: display_name.clone(),
                    account_key: Some(account_key.clone()),
                    lane: Some(lane.lane.clone()),
                    lane_label: Some(lane.label.clone()),
                    period: Some(SessionQuotaPeriodPayload {
                        period_id: period.period_id,
                        starts_at_epoch: period.starts_at_epoch,
                        resets_at_epoch: period.resets_at_epoch,
                        start_source: boundary_source_str(period.start_source).to_string(),
                        reset_source: boundary_source_str(period.reset_source).to_string(),
                    }),
                    usd,
                    percent,
                    confidence,
                    plan: plan.clone(),
                });
            }
        }
    }
    Ok(SessionQuotaPayload {
        entries,
        generated_at: iso_from_epoch(Some(now)),
    })
}

/// A plan is per account, not per lane: the first lane whose newest
/// observation names one speaks for the whole account.
pub(super) fn account_plan(
    store: &Store,
    provider: &str,
    account_key: &str,
    lanes: &[crate::store::provider_limit::QuotaAccountLane],
) -> CommandResult<Option<LiveProviderPlan>> {
    for lane in lanes {
        if let Some((Some(name), tier)) = store
            .latest_observation_plan(provider, account_key, &lane.lane)
            .map_err(fail)?
        {
            return Ok(Some(LiveProviderPlan { name, tier }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::params;

    use super::*;
    use crate::store::AnalysisRecord;
    use crate::store::provider_limit::{FactorPoint, LANE_FIVE_HOUR, LANE_WEEKLY};

    const PROVIDER: &str = "anthropic";
    const AGENT: &str = "claude-code";
    const MODEL: &str = "claude-sonnet-5";

    fn account(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn memory_store() -> Store {
        Store::open_in_memory(Path::new("/tmp/antiburn-quota-commands-test")).expect("opens store")
    }

    fn insert_session(store: &Store, session_id: &str) -> SessionKey {
        let key = SessionKey::new("native", AGENT, session_id);
        store
            .upsert_sessions(
                &[SessionRecord {
                    key: key.clone(),
                    source_kind: "inline".to_string(),
                    source_label: "synthetic".to_string(),
                    wsl_distro: None,
                    title: Some(format!("Session {session_id}")),
                    title_source: None,
                    cwd: None,
                    surface: "unknown".to_string(),
                    updated_at_epoch: Some(1),
                    activity_cursor: "synthetic".to_string(),
                    activity_source: "event".to_string(),
                    subagent_count: 0,
                    fork_parent_session_id: None,
                    source_fingerprint: Some("synthetic".to_string()),
                }],
                &[],
            )
            .expect("stores synthetic session");
        key
    }

    fn insert_turn(store: &Store, key: &SessionKey, ts_ms: i64, input_tokens: i64) {
        let connection = store.lock();
        connection
            .execute(
                "INSERT OR IGNORE INTO session_evidence (
                     environment_key, agent, session_id, status, published_fence
                 ) VALUES (?1, ?2, ?3, 'ready', 1)",
                params![key.environment_key, key.agent, key.session_id],
            )
            .expect("publishes synthetic evidence");
        connection
            .execute(
                "INSERT INTO turn (
                     environment_key, agent, session_id, claim_fence, source_key,
                     thread_id, turn_index, scope, role, ts_ms, model, effort, speed,
                     input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                     is_compaction_boundary, message_id, uuid, parent_uuid
                 ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main', 'assistant',
                           ?4, ?5, NULL, NULL, ?6, 0, 0, 0, 0, NULL, NULL, NULL)",
                params![
                    key.environment_key,
                    key.agent,
                    key.session_id,
                    ts_ms,
                    MODEL,
                    input_tokens
                ],
            )
            .expect("stores synthetic turn");
    }

    fn bind_account(store: &Store, key: &SessionKey, account_key: &str) {
        store
            .lock()
            .execute(
                "INSERT INTO session_provider_account (
                     environment_key, agent, session_id, provider, account_key,
                     provenance, confidence, first_seen_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'provider_live', 'direct', '2026-01-01T00:00:00Z')",
                params![
                    key.environment_key,
                    key.agent,
                    key.session_id,
                    PROVIDER,
                    account_key
                ],
            )
            .expect("binds synthetic account");
    }

    fn seen_account(store: &Store, account_key: &str) {
        store
            .lock()
            .execute(
                "INSERT INTO provider_account_seen (
                     agent, provider, account_key, first_seen_epoch, last_seen_epoch
                 ) VALUES (?1, ?2, ?3, 1, 1)",
                params![AGENT, PROVIDER, account_key],
            )
            .expect("records a seen account");
    }

    fn insert_point(
        store: &Store,
        account_key: &str,
        lane: &str,
        effective_at_epoch: i64,
        usd_per_percent: f64,
    ) {
        store
            .upsert_factor_point(&FactorPoint {
                id: 0,
                provider: PROVIDER.to_string(),
                account_key: account_key.to_string(),
                lane: lane.to_string(),
                effective_at_epoch,
                usd_per_percent,
                method: "delta".to_string(),
                sample_count: 1,
                plan: None,
                plan_tier: None,
            })
            .expect("stores a synthetic factor point");
    }

    fn insert_five_hour_period(store: &Store, account_key: &str, starts: i64, resets: i64) -> i64 {
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', ?3, ?4, ?5, ?6, ?6)",
                params![
                    PROVIDER,
                    account_key,
                    resets - starts,
                    starts,
                    resets,
                    resets
                ],
            )
            .expect("inserts a synthetic period");
        connection.last_insert_rowid()
    }

    /// A weekly period that, like Codex's own reports, states only a reset:
    /// its start is left for the resolver to derive.
    fn insert_weekly_period(store: &Store, account_key: &str, resets_at_epoch: i64) -> i64 {
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'weekly', 'weekly', 'primaryLong',
                           'account', 'account', NULL, NULL, ?3, ?3, ?3)",
                params![PROVIDER, account_key, resets_at_epoch],
            )
            .expect("inserts a synthetic weekly period");
        connection.last_insert_rowid()
    }

    fn insert_observation(
        store: &Store,
        period_id: i64,
        account_key: &str,
        observed_at_epoch: i64,
        used_percent: f64,
    ) {
        insert_observation_with_plan(
            store,
            period_id,
            account_key,
            observed_at_epoch,
            used_percent,
            None,
            None,
        );
    }

    /// Like [`insert_observation`], additionally naming the plan the
    /// synthetic reading reports.
    fn insert_observation_with_plan(
        store: &Store,
        period_id: i64,
        account_key: &str,
        observed_at_epoch: i64,
        used_percent: f64,
        plan: Option<&str>,
        plan_tier: Option<&str>,
    ) {
        store
            .lock()
            .execute(
                "INSERT INTO provider_usage_observation (
                     period_id, provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                     is_authoritative, confidence, source_id, plan, plan_tier
                 ) VALUES (?1, ?2, ?3, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', ?4, ?5, 1, 1, 'high', 'test', ?6, ?7)",
                params![
                    period_id,
                    PROVIDER,
                    account_key,
                    observed_at_epoch,
                    used_percent,
                    plan,
                    plan_tier,
                ],
            )
            .expect("inserts a synthetic observation");
    }

    /// Like [`insert_observation`], for a weekly period instead of a
    /// five-hour one.
    fn insert_weekly_observation(
        store: &Store,
        period_id: i64,
        account_key: &str,
        observed_at_epoch: i64,
        used_percent: f64,
    ) {
        store
            .lock()
            .execute(
                "INSERT INTO provider_usage_observation (
                     period_id, provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                     is_authoritative, confidence, source_id, plan, plan_tier
                 ) VALUES (?1, ?2, ?3, 'weekly', 'weekly', 'primaryLong',
                           'account', 'account', ?4, ?5, 1, 1, 'high', 'test', NULL, NULL)",
                params![
                    period_id,
                    PROVIDER,
                    account_key,
                    observed_at_epoch,
                    used_percent
                ],
            )
            .expect("inserts a synthetic weekly observation");
    }

    fn save_breakdown(store: &Store, key: &SessionKey, input_tokens: u64) {
        let tokens = ModelTokens {
            input_tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: 0,
        };
        let breakdown = std::collections::HashMap::from([(MODEL.to_string(), tokens)]);
        store
            .save_analysis(
                &AnalysisRecord {
                    key: key.clone(),
                    model_breakdown_json: serde_json::to_string(&breakdown)
                        .expect("serializes the breakdown"),
                    pricing_breakdown_json: "{}".to_string(),
                    inclusive_models_json: "[]".to_string(),
                    initial_context_json: None,
                    source_summaries_json: None,
                    provider_hints_json: None,
                    source_fingerprint: "synthetic".to_string(),
                    pricing_generation: 0,
                    analyzed_generation: 0,
                    parser_revision: 0,
                    analyzer_revision: 0,
                    metrics_schema_revision: 0,
                },
                None,
            )
            .expect("saves synthetic analysis");
    }

    #[test]
    fn quota_range_validation_rejects_a_range_over_seventy_days_and_accepts_a_week() {
        assert!(validate_quota_range(0, 7 * 86_400).is_ok());
        assert!(validate_quota_range(0, 70 * 86_400).is_ok());
        assert!(validate_quota_range(0, 71 * 86_400).is_err());
        assert!(
            validate_quota_range(100, 100).is_err(),
            "an empty range is rejected"
        );
    }

    #[test]
    fn get_quota_usage_assigns_buckets_orders_sessions_and_reports_unattributed() {
        let store = memory_store();
        let account_key = account('a');
        let period_id = insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_observation(&store, period_id, &account_key, 0, 0.0);
        insert_observation(&store, period_id, &account_key, 9_000, 10.0);
        insert_observation(&store, period_id, &account_key, 17_000, 30.0);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.5);

        let bound1 = insert_session(&store, "bound1");
        bind_account(&store, &bound1, &account_key);
        insert_turn(&store, &bound1, 100_000, 100_000); // bucket 0

        let bound2 = insert_session(&store, "bound2");
        bind_account(&store, &bound2, &account_key);
        insert_turn(&store, &bound2, 5_000_000, 200_000); // twice the tokens, bucket 4_500

        let unbound = insert_session(&store, "unbound");
        seen_account(&store, &account('a'));
        seen_account(&store, &account('b'));
        insert_turn(&store, &unbound, 10_000_000, 50_000); // ambiguous account

        let payload = quota_usage_for_store(
            &store,
            20_000,
            QuotaUsageRequest {
                provider: PROVIDER.to_string(),
                account_key: account_key.clone(),
                lane: LANE_FIVE_HOUR.to_string(),
                range_start_epoch: 0,
                range_end_epoch: 18_000,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.periods.len(), 1);
        let period = &payload.periods[0];
        assert_eq!(period.period_id, Some(period_id));
        assert_eq!(period.start_source, "reported");
        assert_eq!(period.reset_source, "reported");
        assert_eq!(period.samples.len(), 3);

        let cost_for = |input_tokens: u64| {
            price_breakdown(&std::collections::HashMap::from([(
                MODEL.to_string(),
                ModelTokens {
                    input_tokens,
                    ..Default::default()
                },
            )]))
            .expect("the fixture model is priced")
            .total_usd
        };
        let bound1_usd = cost_for(100_000);
        let bound2_usd = cost_for(200_000);
        let unbound_usd = cost_for(50_000);

        // The three readings (0%, 10%, 20 points later, then another 20
        // points later) turn this period's whole span into two shared
        // segments: [0, 9_000) with a 10-point rise, covering both bound
        // sessions' buckets, and [9_000, 17_000) with a 20-point rise,
        // covering the unbound bucket alone. The new shared-meter model
        // (spec-quota-shared-meter.md) splits each segment's rise by dollars
        // within it, replacing the old flat `usd / factor` division these
        // assertions used before: this is the one existing test the new
        // model actually changes, since it is the only one with readings
        // inside its period.
        let seg1_total_usd = bound1_usd + bound2_usd;
        assert_eq!(period.sessions.len(), 2);
        assert_eq!(period.sessions[0].session_id, "bound2", "descending by usd");
        assert!((period.sessions[0].usd - bound2_usd).abs() < 1e-9);
        assert_eq!(
            period.sessions[0].percent,
            Some(10.0 * bound2_usd / seg1_total_usd)
        );
        assert_eq!(period.sessions[0].title.as_deref(), Some("Session bound2"));
        assert!((period.sessions[1].usd - bound1_usd).abs() < 1e-9);
        assert_eq!(
            period.sessions[1].percent,
            Some(10.0 * bound1_usd / seg1_total_usd)
        );

        assert!((period.unattributed.usd - unbound_usd).abs() < 1e-9);
        // The unbound bucket is the only spend in the second segment, so it
        // carries that segment's whole 20-point rise.
        assert_eq!(period.unattributed.percent, Some(20.0));
        assert_eq!(period.unattributed.session_count, 1);

        // Both segments are fully explained by local dollars, so the total
        // is exactly their combined rise (10 + 20), with nothing unexplained.
        assert_eq!(period.estimated_percent, Some(30.0));
        assert!(period.unexplained_buckets.is_empty());
        assert_eq!(period.unexplained_percent, Some(0.0));

        let bucket_starts: std::collections::BTreeSet<i64> = period
            .contributions
            .iter()
            .map(|contribution| contribution.bucket_start_epoch)
            .collect();
        assert_eq!(
            bucket_starts,
            std::collections::BTreeSet::from([0, 4_500]),
            "only bound contributions carry a bucket; the unbound row does not"
        );

        assert_eq!(
            period.unattributed_buckets.len(),
            1,
            "the unbound session's turn falls in one bucket"
        );
        let unbound_bucket = &period.unattributed_buckets[0];
        assert_eq!(unbound_bucket.bucket_start_epoch, 9_900);
        assert!((unbound_bucket.usd - unbound_usd).abs() < 1e-9);
        assert_eq!(unbound_bucket.percent, Some(20.0));
    }

    /// Two weekly readings that, like Codex's own reports, state only a
    /// reset: the first window's provider-stated reset would run a full
    /// week, but the second window actually began 22 hours in, so the
    /// resolver must cut the first window short there. A bucket that falls
    /// after that cut must credit to the second period, not the first, even
    /// though the first period's unclipped span would also have contained
    /// it.
    #[test]
    fn get_quota_usage_credits_a_bucket_to_the_later_weekly_period_after_truncation() {
        let store = memory_store();
        let account_key = account('w');
        let day = 86_400;
        let first_reset = 7 * day;
        let second_start = 22 * 3_600;
        let second_reset = second_start + 7 * day;
        let first_period_id = insert_weekly_period(&store, &account_key, first_reset);
        let second_period_id = insert_weekly_period(&store, &account_key, second_reset);
        insert_point(&store, &account_key, LANE_WEEKLY, 0, 0.5);

        let session = insert_session(&store, "session");
        bind_account(&store, &session, &account_key);
        let bucket_start = second_start + 8 * 3_600; // the first window's start plus 30h
        insert_turn(&store, &session, bucket_start * 1_000, 100_000);

        let payload = quota_usage_for_store(
            &store,
            second_reset,
            QuotaUsageRequest {
                provider: PROVIDER.to_string(),
                account_key: account_key.clone(),
                lane: LANE_WEEKLY.to_string(),
                range_start_epoch: 0,
                range_end_epoch: second_reset,
            },
        )
        .expect("computes the payload");

        let first = payload
            .periods
            .iter()
            .find(|period| period.period_id == Some(first_period_id))
            .expect("the truncated first period survives");
        assert_eq!(first.reset_source, "truncated");
        assert_eq!(first.resets_at_epoch, second_start);
        assert!(
            first.sessions.is_empty(),
            "the bucket past the cut is not credited to the first period"
        );
        assert!(first.contributions.is_empty());

        let second = payload
            .periods
            .iter()
            .find(|period| period.period_id == Some(second_period_id))
            .expect("the second period is present");
        assert_eq!(
            second.sessions.len(),
            1,
            "the bucket is credited to the second period instead"
        );
        assert_eq!(second.sessions[0].session_id, "session");
        assert!(!second.contributions.is_empty());
    }

    /// A meter rise with no local turns behind it at all: the period has no
    /// factor point either, proving the shared regime needs none. The
    /// whole rise lands in `unexplained_buckets` and `unexplained_percent`
    /// instead of a session or unattributed total.
    #[test]
    fn get_quota_usage_reports_unexplained_for_a_rise_with_no_turns() {
        let store = memory_store();
        let account_key = account('u');
        let period_id = insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_observation(&store, period_id, &account_key, 0, 0.0);
        insert_observation(&store, period_id, &account_key, 9_000, 15.0);

        let payload = quota_usage_for_store(
            &store,
            18_000,
            QuotaUsageRequest {
                provider: PROVIDER.to_string(),
                account_key: account_key.clone(),
                lane: LANE_FIVE_HOUR.to_string(),
                range_start_epoch: 0,
                range_end_epoch: 18_000,
            },
        )
        .expect("computes the payload");

        let period = &payload.periods[0];
        assert!(period.sessions.is_empty());
        assert_eq!(period.unattributed.usd, 0.0);
        assert_eq!(
            period.unexplained_buckets.len(),
            1,
            "the whole rise becomes one unexplained bucket at the segment's end"
        );
        assert_eq!(period.unexplained_buckets[0].bucket_start_epoch, 9_000);
        assert_eq!(period.unexplained_buckets[0].usd, 0.0);
        assert_eq!(period.unexplained_buckets[0].percent, Some(15.0));
        assert_eq!(period.unexplained_percent, Some(15.0));
        assert_eq!(period.estimated_percent, Some(15.0));
    }

    #[test]
    fn get_session_quota_for_a_session_spanning_a_five_hour_reset_returns_two_entries() {
        let store = memory_store();
        let account_key = account('h');
        let key = insert_session(&store, "spanning-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        insert_turn(&store, &key, 100_000, 100_000); // inside [0, 18_000)
        insert_turn(&store, &key, 19_000_000, 100_000); // inside [18_000, 36_000)

        insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_five_hour_period(&store, &account_key, 18_000, 36_000);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.5);

        let payload = session_quota_for_store(
            &store,
            40_000,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "spanning-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(
            payload.entries.len(),
            2,
            "one entry per period the session's turns fell in"
        );
        let mut resets: Vec<i64> = payload
            .entries
            .iter()
            .map(|entry| {
                entry
                    .period
                    .as_ref()
                    .expect("a bound entry carries a period")
                    .resets_at_epoch
            })
            .collect();
        resets.sort_unstable();
        assert_eq!(resets, vec![18_000, 36_000]);
        for entry in &payload.entries {
            assert_eq!(entry.confidence, "learned");
            assert_eq!(entry.account_key.as_deref(), Some(account_key.as_str()));
            assert_eq!(entry.lane.as_deref(), Some(LANE_FIVE_HOUR));
            assert!(entry.usd > 0.0);
            assert!(entry.percent.is_some());
        }
    }

    /// One period whose readings cover the session's own turn end to end
    /// reports `"measured"`; a second period with no readings at all, where
    /// the same session's turn falls in the estimated tail, keeps today's
    /// factor confidence.
    #[test]
    fn get_session_quota_returns_measured_inside_coverage_and_the_factor_confidence_in_the_tail() {
        let store = memory_store();
        let account_key = account('m');
        let key = insert_session(&store, "measured-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        insert_turn(&store, &key, 100_000, 100_000); // inside [0, 18_000), bucket 0
        insert_turn(&store, &key, 19_000_000, 100_000); // inside [18_000, 36_000), no readings

        let covered_period_id = insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_observation(&store, covered_period_id, &account_key, 0, 0.0);
        insert_observation(&store, covered_period_id, &account_key, 9_000, 20.0);
        insert_five_hour_period(&store, &account_key, 18_000, 36_000);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.5);

        let payload = session_quota_for_store(
            &store,
            40_000,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "measured-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 2);
        let covered = payload
            .entries
            .iter()
            .find(|entry| entry.period.as_ref().unwrap().resets_at_epoch == 18_000)
            .expect("the covered period's entry");
        assert_eq!(covered.confidence, "measured");
        // The session is the only spend in its segment, so it carries the
        // segment's whole 20-point rise.
        assert_eq!(covered.percent, Some(20.0));

        let tail = payload
            .entries
            .iter()
            .find(|entry| entry.period.as_ref().unwrap().resets_at_epoch == 36_000)
            .expect("the tail period's entry");
        assert_eq!(tail.confidence, "learned");
        assert!(tail.percent.is_some());
    }

    /// A session with one bucket in the shared segment and one past the
    /// last reading, in a lane that has never learned a factor point at
    /// all: the entry is kept, not dropped, reporting the shared percent
    /// and `"measured"` rather than losing an already-priced contribution
    /// for lack of a factor the unpriced tail alone would have needed.
    #[test]
    fn get_session_quota_keeps_a_measured_entry_whose_tail_has_no_factor() {
        let store = memory_store();
        let account_key = account('k');
        let key = insert_session(&store, "no-factor-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        insert_turn(&store, &key, 100_000, 100_000); // inside [0, 9_000), shared regime
        insert_turn(&store, &key, 19_000_000, 100_000); // past the last reading, no factor

        let period_id = insert_five_hour_period(&store, &account_key, 0, 40_000);
        insert_observation(&store, period_id, &account_key, 0, 0.0);
        insert_observation(&store, period_id, &account_key, 9_000, 20.0);
        // No insert_point call: the lane has never learned a factor.

        let payload = session_quota_for_store(
            &store,
            40_000,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "no-factor-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 1);
        let entry = &payload.entries[0];
        assert_eq!(entry.confidence, "measured");
        // The tail bucket has no factor to price it, so the entry carries
        // only the shared segment's own percent.
        assert_eq!(entry.percent, Some(20.0));
    }

    /// A session whose turns are 100 days apart: querying every weekly
    /// period back to the session's own start would pull in a period from
    /// months ago. The card instead clamps its range to
    /// `MAX_QUOTA_RANGE_DAYS`, so the payload names only the window around
    /// the recent turn.
    #[test]
    fn get_session_quota_bounds_a_session_spanning_a_hundred_days_to_the_recent_window() {
        let store = memory_store();
        let account_key = account('q');
        let key = insert_session(&store, "long-lived-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        let day = 86_400;
        let old_epoch = 100;
        let recent_epoch = old_epoch + 100 * day;
        insert_turn(&store, &key, old_epoch * 1_000, 100_000);
        insert_turn(&store, &key, recent_epoch * 1_000, 100_000);

        let old_reset = 7 * day;
        let recent_reset = recent_epoch + 100;
        let old_period_id = insert_weekly_period(&store, &account_key, old_reset);
        let recent_period_id = insert_weekly_period(&store, &account_key, recent_reset);
        insert_weekly_observation(&store, old_period_id, &account_key, 0, 0.0);
        insert_weekly_observation(&store, old_period_id, &account_key, old_epoch, 10.0);
        insert_weekly_observation(
            &store,
            recent_period_id,
            &account_key,
            recent_reset - day,
            0.0,
        );
        insert_weekly_observation(&store, recent_period_id, &account_key, recent_epoch, 10.0);
        insert_point(&store, &account_key, LANE_WEEKLY, 0, 0.5);

        let payload = session_quota_for_store(
            &store,
            recent_reset,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "long-lived-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        for entry in &payload.entries {
            assert_eq!(
                entry.period.as_ref().unwrap().resets_at_epoch,
                recent_reset,
                "the payload names only the recent window, not the one 100 days back"
            );
        }
        assert!(
            payload
                .entries
                .iter()
                .any(|entry| entry.period.as_ref().unwrap().resets_at_epoch == recent_reset),
            "the recent window is still reported"
        );
    }

    #[test]
    fn get_session_quota_for_an_unbound_session_reports_one_unbound_entry() {
        let store = memory_store();
        let key = insert_session(&store, "unbound-session");
        save_breakdown(&store, &key, 1_000_000);
        seen_account(&store, &account('x'));
        seen_account(&store, &account('y'));
        insert_turn(&store, &key, 100_000, 100_000);

        let payload = session_quota_for_store(
            &store,
            1_000,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "unbound-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 1);
        let entry = &payload.entries[0];
        assert_eq!(entry.confidence, "unbound");
        assert_eq!(entry.account_key, None);
        assert_eq!(entry.lane, None);
        assert_eq!(entry.lane_label, None);
        assert_eq!(entry.percent, None);
        assert!(entry.period.is_none());
        assert!(entry.usd > 0.0);
    }

    /// The account's newest observation names its plan; every bound entry
    /// for that account carries it, since a plan applies to the whole
    /// account, not to one lane.
    #[test]
    fn get_session_quota_names_the_accounts_plan_from_its_newest_observation() {
        let store = memory_store();
        let account_key = account('p');
        let key = insert_session(&store, "plan-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        insert_turn(&store, &key, 100_000, 100_000); // inside [0, 18_000)
        let period_id = insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_observation_with_plan(
            &store,
            period_id,
            &account_key,
            0,
            0.0,
            Some("max"),
            Some("max_20x"),
        );
        insert_observation_with_plan(
            &store,
            period_id,
            &account_key,
            9_000,
            20.0,
            Some("max"),
            Some("max_20x"),
        );

        let payload = session_quota_for_store(
            &store,
            20_000,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "plan-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 1);
        let plan = payload.entries[0].plan.as_ref().expect("names the plan");
        assert_eq!(plan.name, "max");
        assert_eq!(plan.tier.as_deref(), Some("max_20x"));
    }

    /// An account with no observation naming a plan yet reports `None`,
    /// rather than guessing one.
    #[test]
    fn get_session_quota_leaves_the_plan_none_before_any_observation_names_one() {
        let store = memory_store();
        let account_key = account('q');
        let key = insert_session(&store, "no-plan-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        insert_turn(&store, &key, 100_000, 100_000); // inside [0, 18_000)
        insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.5);

        let payload = session_quota_for_store(
            &store,
            20_000,
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "no-plan-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 1);
        assert_eq!(payload.entries[0].plan, None);
    }

    /// A closed period with no readings at all, whose factor prices its two
    /// sessions' turns to a combined 120 percent: the period caps at exactly
    /// 100, and the two sessions' 3:1 dollar ratio still holds, so they land
    /// at 75 and 25.
    #[test]
    fn get_quota_usage_caps_a_closed_period_with_no_readings_and_scales_sessions_by_dollars() {
        let store = memory_store();
        let account_key = account('c');
        insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.01);

        let bound1 = insert_session(&store, "cap-bound1");
        bind_account(&store, &bound1, &account_key);
        insert_turn(&store, &bound1, 100_000, 300_000); // bucket 0, 3 parts

        let bound2 = insert_session(&store, "cap-bound2");
        bind_account(&store, &bound2, &account_key);
        insert_turn(&store, &bound2, 5_000_000, 100_000); // bucket 4_500, 1 part

        let payload = quota_usage_for_store(
            &store,
            20_000, // now: past the reset, so the period is closed
            QuotaUsageRequest {
                provider: PROVIDER.to_string(),
                account_key: account_key.clone(),
                lane: LANE_FIVE_HOUR.to_string(),
                range_start_epoch: 0,
                range_end_epoch: 18_000,
            },
        )
        .expect("computes the payload");

        let period = &payload.periods[0];
        assert_eq!(
            period.estimated_percent,
            Some(100.0),
            "the closed period's overshoot caps at exactly 100"
        );
        assert_eq!(period.sessions.len(), 2);
        let bound1_session = period
            .sessions
            .iter()
            .find(|session| session.session_id == "cap-bound1")
            .expect("the first session's entry");
        let bound2_session = period
            .sessions
            .iter()
            .find(|session| session.session_id == "cap-bound2")
            .expect("the second session's entry");
        assert!((bound1_session.percent.unwrap() - 75.0).abs() < 1e-6);
        assert!((bound2_session.percent.unwrap() - 25.0).abs() < 1e-6);
    }

    /// The same period and sessions as above, but still open (its reset
    /// falls after `now`): the raw factor overshoot returns unchanged, since
    /// an open period can still gather more readings before it closes.
    #[test]
    fn get_quota_usage_leaves_an_open_periods_overshoot_uncapped() {
        let store = memory_store();
        let account_key = account('o');
        insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.01);

        let bound1 = insert_session(&store, "open-bound1");
        bind_account(&store, &bound1, &account_key);
        insert_turn(&store, &bound1, 100_000, 300_000);

        let bound2 = insert_session(&store, "open-bound2");
        bind_account(&store, &bound2, &account_key);
        insert_turn(&store, &bound2, 5_000_000, 100_000);

        let cost_for = |input_tokens: u64| {
            price_breakdown(&std::collections::HashMap::from([(
                MODEL.to_string(),
                ModelTokens {
                    input_tokens,
                    ..Default::default()
                },
            )]))
            .expect("the fixture model is priced")
            .total_usd
        };
        let expected_overshoot = (cost_for(300_000) + cost_for(100_000)) / 0.01;
        assert!(expected_overshoot > 100.0, "the fixture must overshoot 100");

        let payload = quota_usage_for_store(
            &store,
            10_000, // now: before the reset, so the period is still open
            QuotaUsageRequest {
                provider: PROVIDER.to_string(),
                account_key: account_key.clone(),
                lane: LANE_FIVE_HOUR.to_string(),
                range_start_epoch: 0,
                range_end_epoch: 18_000,
            },
        )
        .expect("computes the payload");

        let period = &payload.periods[0];
        assert!(
            (period.estimated_percent.unwrap() - expected_overshoot).abs() < 1e-6,
            "an open period keeps its raw overshoot past 100"
        );
    }

    /// A closed period with no readings at all, where a single session is
    /// the sole dollar source: capping scales its own stack to exactly 100,
    /// but the session still reports the factor's own confidence, not
    /// `"measured"`, since nothing here came from a real meter reading.
    #[test]
    fn get_session_quota_reports_factor_confidence_not_measured_in_a_capped_tail() {
        let store = memory_store();
        let account_key = account('n');
        let key = insert_session(&store, "capped-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        insert_turn(&store, &key, 100_000, 300_000); // inside [0, 18_000), no readings
        insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.001);

        let payload = session_quota_for_store(
            &store,
            20_000, // now: past the reset, so the period is closed
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "capped-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 1);
        let entry = &payload.entries[0];
        assert_eq!(
            entry.confidence, "learned",
            "a capped tail reports the factor's confidence, not measured"
        );
        assert!((entry.percent.unwrap() - 100.0).abs() < 1e-6);
    }

    /// A closed five-hour window that starts at 1,020 (a 7:27-style offset,
    /// not on a 15-minute boundary) instead of 0: its last absolute
    /// 15-minute bucket, 18,900 to 19,800, straddles the reset at 19,020,
    /// so only 120 of its 900 seconds belong to the window. The window's
    /// sole spend lands in that bucket, and the factor prices it well past
    /// 100. With the bucket's tail correctly stopping at the reset instead
    /// of running past it, the session's own percent still lands at 100,
    /// not above it.
    #[test]
    fn get_session_quota_caps_a_session_whose_start_is_off_the_bucket_grid() {
        let store = memory_store();
        let account_key = account('o');
        let key = insert_session(&store, "offset-start-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);

        // ts 19,000s falls in the absolute bucket started at 18,900s, which
        // straddles the window's reset at 19,020s.
        insert_turn(&store, &key, 19_000_000, 300_000);
        insert_five_hour_period(&store, &account_key, 1_020, 19_020);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 0.001);

        let payload = session_quota_for_store(
            &store,
            20_000, // now: past the reset, so the window is closed
            SessionQuotaRequest {
                agent: AGENT.to_string(),
                session_id: "offset-start-session".to_string(),
                wsl_distro: None,
            },
        )
        .expect("computes the payload");

        assert_eq!(payload.entries.len(), 1);
        let entry = &payload.entries[0];
        assert!(
            entry.percent.unwrap() <= 100.0 + 1e-6,
            "a straddling tail bucket must not push the session's percent past 100, got {}",
            entry.percent.unwrap()
        );
    }

    #[test]
    fn overview_allowance_uses_the_same_account_wide_period_as_limits() {
        const DAY: i64 = 86_400;
        let now = 70 * DAY;
        let store = memory_store();
        let account_key = account('v');
        let reset = now + DAY;
        let period_id = insert_weekly_period(&store, &account_key, reset);
        insert_weekly_observation(&store, period_id, &account_key, now - 3 * DAY, 0.0);
        insert_weekly_observation(&store, period_id, &account_key, now - DAY, 20.0);
        insert_weekly_observation(&store, period_id, &account_key, now - 12 * 3_600, 50.0);

        let bound = insert_session(&store, "overview-bound");
        bind_account(&store, &bound, &account_key);
        insert_turn(&store, &bound, (now - 2 * DAY) * 1_000, 100_000);
        let unbound = insert_session(&store, "overview-unbound");
        seen_account(&store, &account_key);
        seen_account(&store, &account('w'));
        insert_turn(&store, &unbound, (now - 2 * DAY + 3_600) * 1_000, 100_000);

        let fable_period_id = {
            let connection = store.lock();
            connection
                .execute(
                    "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'weekly-fable', 'weekly', 'supplemental',
                           'model:fable', 'Fable', NULL, NULL, ?3, ?3, ?3)",
                    params![PROVIDER, account_key, reset],
                )
                .expect("adds a model-scoped lane");
            connection.last_insert_rowid()
        };
        store
            .lock()
            .execute(
                "INSERT INTO provider_usage_observation (
                     period_id, provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                     is_authoritative, confidence, source_id, plan, plan_tier
                 ) VALUES (?1, ?2, ?3, 'weekly-fable', 'weekly', 'supplemental',
                           'model:fable', 'Fable', ?4, ?5, 1, 1, 'high', 'test', NULL, NULL)",
                params![fable_period_id, PROVIDER, account_key, now - DAY, 40.0],
            )
            .expect("inserts a synthetic model-scoped observation");

        let bounds = crate::provider_usage::window_bounds(now, 0);
        let limits = quota_usage_for_store(
            &store,
            now,
            QuotaUsageRequest {
                provider: PROVIDER.to_string(),
                account_key: account_key.clone(),
                lane: LANE_WEEKLY.to_string(),
                range_start_epoch: bounds.previous_30_days_start - 7 * DAY,
                range_end_epoch: now + 1,
            },
        )
        .expect("Limits computes the weekly period");
        let shared = limits
            .periods
            .iter()
            .find(|period| period.period_id == Some(period_id))
            .expect("shared period exists");
        assert_eq!(shared.estimated_percent, Some(50.0));
        assert!(!shared.contributions.is_empty());
        assert!(!shared.unattributed_buckets.is_empty());
        assert_eq!(shared.unexplained_percent, Some(30.0));

        let overview = super::super::usage::allowance_usage_for_store(&store, now, 0)
            .expect("Overview computes the same account");
        let account = overview
            .accounts
            .iter()
            .find(|account| account.account_key == account_key)
            .expect("account appears");
        assert_eq!(overview.range_start_epoch, bounds.last_30_days_start);
        assert_eq!(overview.range_end_epoch, now);
        assert_eq!(overview.utilization_span_days, 28);

        // The window opened less than seven days ago (it resets a day from
        // now), so the rolling line has not reached its start yet and the
        // headline states no figure.
        assert_eq!(account.utilization, None);

        // The chart still draws both weekly-style areas, from the same
        // shared period Limits computed above: its last level equals
        // `estimated_percent`, because both sum the same buckets.
        let weekly_area = account
            .chart
            .weekly_windows
            .iter()
            .find(|window| window.lane == "weekly")
            .expect("the account-wide weekly window draws");
        assert_eq!(
            weekly_area.points.last().map(|point| point.percent),
            shared.estimated_percent
        );
        assert!(
            account
                .chart
                .weekly_windows
                .iter()
                .any(|window| window.lane == "model:fable"),
            "the model-scoped window draws its own area"
        );
    }
    #[test]
    fn session_and_limits_use_actual_turn_times_on_both_sides_of_a_reading() {
        let store = memory_store();
        let account_key = account('t');
        let key = insert_session(&store, "timed-session");
        let other = insert_session(&store, "other-session");
        for session in [&key, &other] {
            bind_account(&store, session, &account_key);
            save_breakdown(&store, session, 1_000_000);
        }
        let period_id = insert_five_hour_period(&store, &account_key, 0, 18_000);
        insert_observation(&store, period_id, &account_key, 289, 92.0);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 1.0);
        insert_turn(&store, &key, 100_000, 300_000);
        insert_turn(&store, &other, 200_000, 100_000);
        let request = || SessionQuotaRequest {
            agent: AGENT.into(),
            session_id: key.session_id.clone(),
            wsl_distro: None,
        };
        let limits_request = || QuotaUsageRequest {
            provider: PROVIDER.into(),
            account_key: account_key.clone(),
            lane: LANE_FIVE_HOUR.into(),
            range_start_epoch: 0,
            range_end_epoch: 18_000,
        };
        let initial = session_quota_for_store(&store, 300, request()).unwrap();
        assert_eq!(initial.entries[0].percent, Some(69.0));
        let limits = quota_usage_for_store(&store, 300, limits_request()).unwrap();
        assert_eq!(limits.periods[0].estimated_percent, Some(92.0));

        insert_turn(&store, &key, 300_001, 100_000);
        insert_turn(&store, &key, 500_000, 900_000);
        let updated = session_quota_for_store(&store, 310, request()).unwrap();
        let limits = quota_usage_for_store(&store, 310, limits_request()).unwrap();
        let session = limits.periods[0]
            .sessions
            .iter()
            .find(|s| s.session_id == key.session_id)
            .unwrap();
        assert_eq!(updated.entries[0].percent, session.percent);
        assert_eq!(updated.entries[0].usd, session.usd);
        let tail_usd = session.usd / 4.0;
        assert!((session.percent.unwrap() - (69.0 + tail_usd)).abs() < 1e-9);
        assert_eq!(updated.entries[0].confidence, "learned");
        assert!((limits.periods[0].estimated_percent.unwrap() - (92.0 + tail_usd)).abs() < 1e-9);

        insert_observation(&store, period_id, &account_key, 310, 94.0);
        let refreshed = session_quota_for_store(&store, 310, request()).unwrap();
        assert_eq!(refreshed.entries[0].percent, Some(71.0));
        let limits = quota_usage_for_store(&store, 310, limits_request()).unwrap();
        assert_eq!(limits.periods[0].estimated_percent, Some(94.0));
    }

    #[test]
    fn session_and_limits_split_a_bucket_at_an_off_grid_reset() {
        let store = memory_store();
        let account_key = account('r');
        let key = insert_session(&store, "reset-session");
        bind_account(&store, &key, &account_key);
        save_breakdown(&store, &key, 1_000_000);
        let first = insert_five_hour_period(&store, &account_key, 120, 18_120);
        let second = insert_five_hour_period(&store, &account_key, 18_120, 36_120);
        insert_turn(&store, &key, 18_119_999, 100_000);
        insert_turn(&store, &key, 18_120_000, 300_000);
        insert_observation(&store, first, &account_key, 18_119, 20.0);
        insert_observation(&store, second, &account_key, 18_200, 10.0);
        insert_point(&store, &account_key, LANE_FIVE_HOUR, 0, 1.0);
        let now = 18_300;
        let session = session_quota_for_store(
            &store,
            now,
            SessionQuotaRequest {
                agent: AGENT.into(),
                session_id: key.session_id.clone(),
                wsl_distro: None,
            },
        )
        .unwrap();
        let limits = quota_usage_for_store(
            &store,
            now,
            QuotaUsageRequest {
                provider: PROVIDER.into(),
                account_key,
                lane: LANE_FIVE_HOUR.into(),
                range_start_epoch: 120,
                range_end_epoch: 36_120,
            },
        )
        .unwrap();
        assert_eq!(session.entries.len(), 2);
        for entry in &session.entries {
            let period = limits
                .periods
                .iter()
                .find(|p| p.period_id == entry.period.as_ref().unwrap().period_id)
                .unwrap();
            assert_eq!(period.sessions.len(), 1);
            assert_eq!(entry.usd, period.sessions[0].usd);
            assert_eq!(entry.percent, period.sessions[0].percent);
        }
        assert_eq!(limits.periods[1].sessions[0].percent, Some(10.0));
        assert!(
            (limits.periods[1].sessions[0].usd - 3.0 * limits.periods[0].sessions[0].usd).abs()
                < 1e-9
        );
    }
}
