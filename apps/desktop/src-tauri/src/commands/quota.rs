//! Quota period, meter, and contribution queries.
//!
//! These commands read the periods a quota lane ran through, its meter
//! readings, and the sessions estimated to have contributed to each period's
//! spend. See `crate::provider_usage::quota` for the boundary resolver and
//! `crate::store::provider_limit` for the underlying store queries.

use super::*;
use crate::dto::{
    QuotaAccountPayload, QuotaAccountsPayload, QuotaContributionPayload, QuotaCurrentPeriodPayload,
    QuotaFactorPayload, QuotaLanePayload, QuotaPeriodPayload, QuotaSamplePayload,
    QuotaSessionTotalPayload, QuotaUnattributedPayload, QuotaUsagePayload, QuotaUsageRequest,
    SessionQuotaEntryPayload, SessionQuotaPayload, SessionQuotaPeriodPayload, SessionQuotaRequest,
};

/// A quota query's range may not exceed this many days: enough for a month
/// view, bounded so one request cannot force an unbounded turn-row scan.
const MAX_QUOTA_RANGE_DAYS: i64 = 35;

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
                has_factor: lane.has_factor,
                current_period: lane
                    .current_period
                    .map(
                        |(starts_at_epoch, resets_at_epoch)| QuotaCurrentPeriodPayload {
                            starts_at_epoch,
                            resets_at_epoch,
                        },
                    ),
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
fn quota_usage_for_store(
    store: &Store,
    now: i64,
    request: QuotaUsageRequest,
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
        store
            .attributed_turn_epochs(&provider, &account_key, range_start_epoch, range_end_epoch)
            .map_err(fail)?
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
    let has_factor = !points.is_empty();
    let factor = points.last().map(|point| QuotaFactorPayload {
        usd_per_percent: point.usd_per_percent,
        confidence: factor_confidence(point).to_string(),
    });

    let bucketed = store
        .attributed_turn_dollars_by_bucket(
            &provider,
            &account_key,
            model_scope,
            range_start_epoch,
            range_end_epoch,
        )
        .map_err(fail)?
        .ok_or_else(|| "too much turn activity in this range".to_string())?;

    // Assign each bucket to the period whose `[start, reset)` contains its
    // start. A bucket that straddles a reset lands in the period containing
    // its start, never the one its tail spills into.
    let mut by_period: Vec<Vec<&crate::store::provider_limit::BucketedSessionDollars>> =
        vec![Vec::new(); periods.len()];
    for row in &bucketed {
        if let Some(index) = periods.iter().position(|period| {
            row.bucket_start_epoch >= period.starts_at_epoch
                && row.bucket_start_epoch < period.resets_at_epoch
        }) {
            by_period[index].push(row);
        }
    }

    let session_keys: HashSet<SessionKey> = bucketed.iter().map(|row| row.key.clone()).collect();
    let session_titles = session_titles_and_distros(store, &session_keys)?;

    let mut period_payloads = Vec::with_capacity(periods.len());
    for (period, rows) in periods.iter().zip(by_period) {
        let samples: Vec<QuotaSamplePayload> = match period.period_id {
            Some(period_id) => store
                .quota_period_samples(period_id)
                .map_err(fail)?
                .into_iter()
                .map(|observation| QuotaSamplePayload {
                    observed_at_epoch: observation.observed_at_epoch,
                    used_percent: observation.used_percent,
                    fresh: observation.is_fresh,
                    authoritative: observation.is_authoritative,
                })
                .collect(),
            None => Vec::new(),
        };
        let peak_percent = samples
            .iter()
            .filter(|sample| sample.authoritative)
            .filter_map(|sample| sample.used_percent)
            .fold(None, |max: Option<f64>, value| {
                Some(max.map_or(value, |max| max.max(value)))
            });

        let mut contributions = Vec::new();
        let mut per_session_bound: HashMap<SessionKey, (f64, f64)> = HashMap::new();
        let mut unattributed_usd = 0.0;
        let mut unattributed_sessions: HashSet<SessionKey> = HashSet::new();
        for row in rows {
            let bucket_end =
                row.bucket_start_epoch + crate::store::provider_limit::CONTRIBUTION_BUCKET_SECS;
            let percent =
                crate::provider_usage::quota::factor_point_at_or_earliest(&points, bucket_end)
                    .map(|point| row.usd / point.usd_per_percent);
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
                        .or_insert((0.0, 0.0));
                    entry.0 += row.usd;
                    entry.1 += percent.unwrap_or(0.0);
                }
                crate::store::provider_limit::Resolved::Unbound => {
                    unattributed_usd += row.usd;
                    unattributed_sessions.insert(row.key.clone());
                }
            }
        }
        let mut sessions: Vec<QuotaSessionTotalPayload> = per_session_bound
            .into_iter()
            .map(|(key, (usd, percent))| {
                let (title, wsl_distro) = session_titles.get(&key).cloned().unwrap_or_default();
                QuotaSessionTotalPayload {
                    agent: key.agent.clone(),
                    session_id: key.session_id.clone(),
                    wsl_distro,
                    title,
                    usd,
                    percent: has_factor.then_some(percent),
                }
            })
            .collect();
        sessions.sort_by(|left, right| right.usd.total_cmp(&left.usd));
        let estimated_percent =
            has_factor.then(|| sessions.iter().filter_map(|session| session.percent).sum());
        let unattributed_percent = has_factor
            .then(|| {
                crate::provider_usage::quota::factor_point_at_or_earliest(
                    &points,
                    period.resets_at_epoch,
                )
                .map(|point| unattributed_usd / point.usd_per_percent)
            })
            .flatten();

        period_payloads.push(QuotaPeriodPayload {
            period_id: period.period_id,
            starts_at_epoch: period.starts_at_epoch,
            resets_at_epoch: period.resets_at_epoch,
            start_source: boundary_source_str(period.start_source).to_string(),
            reset_source: boundary_source_str(period.reset_source).to_string(),
            samples,
            peak_percent,
            contributions,
            sessions,
            unattributed: QuotaUnattributedPayload {
                usd: unattributed_usd,
                percent: unattributed_percent,
                session_count: unattributed_sessions.len() as u32,
            },
            estimated_percent,
        });
    }

    Ok(QuotaUsagePayload {
        provider,
        account_key,
        lane,
        lane_label,
        range_start_epoch,
        range_end_epoch,
        factor,
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
                    lane: crate::store::provider_limit::LANE_WEEKLY.to_string(),
                    lane_label: "Weekly".to_string(),
                    period: None,
                    usd,
                    percent: None,
                    confidence: "unbound".to_string(),
                });
            }
            continue;
        };

        let accounts = store.quota_accounts(now).map_err(fail)?;
        let Some(account) = accounts
            .into_iter()
            .find(|account| account.provider == provider && account.account_key == account_key)
        else {
            continue;
        };
        for lane in account.lanes {
            let lane_duration = crate::store::provider_limit::lane_duration_seconds(&lane.lane);
            let range_start = min_epoch - lane_duration;
            let range_end = max_epoch + 1;
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
                    .attributed_turn_dollars_between(
                        provider,
                        &account_key,
                        period.starts_at_epoch,
                        period.resets_at_epoch,
                        model_scope,
                    )
                    .map_err(fail)?
                else {
                    continue;
                };
                let Some(session_dollars) = dollars.iter().find(|row| row.key == key) else {
                    continue;
                };
                let usd = session_dollars.input_usd
                    + session_dollars.output_usd
                    + session_dollars.cache_read_usd
                    + session_dollars.cache_write_usd;
                if usd <= 0.0 {
                    continue;
                }
                let Some(point) = crate::provider_usage::quota::factor_point_at_or_earliest(
                    &points,
                    period.resets_at_epoch.min(now),
                ) else {
                    continue;
                };
                let samples = match period.period_id {
                    Some(period_id) => store.quota_period_samples(period_id).map_err(fail)?,
                    None => Vec::new(),
                };
                let peak_percent = samples
                    .iter()
                    .filter(|observation| observation.is_authoritative)
                    .filter_map(|observation| observation.used_percent)
                    .fold(None, |max: Option<f64>, value| {
                        Some(max.map_or(value, |max| max.max(value)))
                    });
                entries.push(SessionQuotaEntryPayload {
                    provider: provider.to_string(),
                    display_name: display_name.clone(),
                    account_key: Some(account_key.clone()),
                    lane: lane.lane.clone(),
                    lane_label: lane.label.clone(),
                    period: Some(SessionQuotaPeriodPayload {
                        period_id: period.period_id,
                        starts_at_epoch: period.starts_at_epoch,
                        resets_at_epoch: period.resets_at_epoch,
                        start_source: boundary_source_str(period.start_source).to_string(),
                        reset_source: boundary_source_str(period.reset_source).to_string(),
                        peak_percent,
                    }),
                    usd,
                    percent: Some(usd / point.usd_per_percent),
                    confidence: factor_confidence(point).to_string(),
                });
            }
        }
    }
    Ok(SessionQuotaPayload {
        entries,
        generated_at: iso_from_epoch(Some(now)),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::params;

    use super::*;
    use crate::store::AnalysisRecord;
    use crate::store::provider_limit::{FactorPoint, LANE_FIVE_HOUR};

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

    fn insert_observation(
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
                     is_authoritative, confidence, source_id
                 ) VALUES (?1, ?2, ?3, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', ?4, ?5, 1, 1, 'high', 'test')",
                params![
                    period_id,
                    PROVIDER,
                    account_key,
                    observed_at_epoch,
                    used_percent
                ],
            )
            .expect("inserts a synthetic observation");
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
    fn quota_range_validation_rejects_a_range_over_thirty_five_days_and_accepts_a_week() {
        assert!(validate_quota_range(0, 7 * 86_400).is_ok());
        assert!(validate_quota_range(0, 36 * 86_400).is_err());
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
        assert_eq!(period.peak_percent, Some(30.0));

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

        assert_eq!(period.sessions.len(), 2);
        assert_eq!(period.sessions[0].session_id, "bound2", "descending by usd");
        assert!((period.sessions[0].usd - bound2_usd).abs() < 1e-9);
        assert_eq!(period.sessions[0].percent, Some(bound2_usd / 0.5));
        assert_eq!(period.sessions[0].title.as_deref(), Some("Session bound2"));
        assert!((period.sessions[1].usd - bound1_usd).abs() < 1e-9);
        assert_eq!(period.sessions[1].percent, Some(bound1_usd / 0.5));

        assert!((period.unattributed.usd - unbound_usd).abs() < 1e-9);
        assert_eq!(period.unattributed.percent, Some(unbound_usd / 0.5));
        assert_eq!(period.unattributed.session_count, 1);

        assert_eq!(
            period.estimated_percent,
            Some(bound1_usd / 0.5 + bound2_usd / 0.5)
        );
        assert_eq!(
            payload.factor.as_ref().map(|factor| factor.usd_per_percent),
            Some(0.5)
        );
        assert_eq!(
            payload
                .factor
                .as_ref()
                .map(|factor| factor.confidence.clone()),
            Some("learned".to_string())
        );

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
            assert_eq!(entry.lane, LANE_FIVE_HOUR);
            assert!(entry.usd > 0.0);
            assert!(entry.percent.is_some());
        }
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
        assert_eq!(entry.percent, None);
        assert!(entry.period.is_none());
        assert!(entry.usd > 0.0);
    }
}
