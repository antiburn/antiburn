use super::*;
use antiburn_local::analysis::{SourceOrigin, lookup_turn_pricing, pricing_generation};
use antiburn_local::insights::{
    SessionTokenBurnEvidence, TokenBurnSourceEvidence, TokenBurnTurnAccumulator,
    TokenBurnTurnEvidence,
};
use antiburn_local::pricing::canonical_model_key;

pub(super) struct TokenBurnSessionKey<'a> {
    pub environment_key: &'a str,
    pub agent: &'a str,
    pub session_id: &'a str,
    pub published_fence: i64,
    pub cwd: Option<&'a str>,
}

struct SourceTokenCounter {
    evidence: TokenBurnSourceEvidence,
    definition_tokens: u128,
}

pub(super) struct TokenBurnReportContext<'a> {
    pub catalogs: &'a ReportCatalogs,
    pub depth_cap: u128,
}

pub(super) struct TokenBurnProbes<'a> {
    pub turn: &'a mut dyn FnMut(),
    pub resource_turn: &'a mut dyn FnMut(u128),
}

pub(super) fn token_burn_evidence(
    connection: &rusqlite::Connection,
    key: TokenBurnSessionKey<'_>,
    initial_context: Option<&InitialContextBreakdown>,
    evidence: &SessionEvidence,
    report_context: &TokenBurnReportContext<'_>,
    cancel: &AtomicBool,
    probes: &mut TokenBurnProbes<'_>,
) -> Result<SessionTokenBurnEvidence> {
    // The partial index limits row discovery to this session's assistant turns.
    // This build omits rusqlite hooks, so probes run per row and before finalization.
    let mut statement = connection.prepare_cached(TOKEN_BURN_TURNS_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        key.published_fence
    ])?;
    let mut source_groups = initial_context.map(|initial_context| {
        [
            source_token_counters(initial_context, "mcp_instructions", key.agent, key.cwd),
            source_token_counters(initial_context, "builtin_tool", key.agent, key.cwd)
                .filter(|sources| !sources.is_empty()),
            source_token_counters(initial_context, "skill_instructions", key.agent, key.cwd),
        ]
    });
    let mut turn_accumulator = TokenBurnTurnAccumulator::new(report_context.catalogs);
    let mut has_unattributed_assistant_turn = false;
    let mut raw_total_tokens = 0_u128;
    let mut overdepth_avoidable_tokens = 0_u128;
    while let Some(row) = rows.next()? {
        (probes.turn)();
        ensure_not_cancelled(cancel)?;
        let scope: String = row.get(0)?;
        let model: Option<String> = row.get(1)?;
        let effort: Option<String> = row.get(2)?;
        let speed: Option<String> = row.get(3)?;
        let ts_ms: Option<i64> = row.get(4)?;
        let input_tokens = u64::try_from(row.get::<_, i64>(5)?)?;
        let output_tokens = u64::try_from(row.get::<_, i64>(6)?)?;
        let cache_read_tokens = u64::try_from(row.get::<_, i64>(7)?)?;
        let cache_write_tokens = u64::try_from(row.get::<_, i64>(8)?)?;
        let cache_write_1h_tokens = u64::try_from(row.get::<_, i64>(9)?)?;
        let input = u128::from(input_tokens);
        let output = u128::from(output_tokens);
        let cache_read = u128::from(cache_read_tokens);
        let cache_write = u128::from(cache_write_tokens);
        let context = input
            .checked_add(cache_read)
            .and_then(|value| value.checked_add(cache_write))
            .context("turn context token total overflowed")?;
        let turn_total = context
            .checked_add(output)
            .context("turn token total overflowed")?;
        if scope == "main" || scope == "delegated" {
            (probes.resource_turn)(context);
        }
        let Some(model) = model.filter(|model| !model.trim().is_empty()) else {
            has_unattributed_assistant_turn = true;
            raw_total_tokens = raw_total_tokens
                .checked_add(turn_total)
                .context("session token total overflowed")?;
            continue;
        };
        let turn = TokenBurnTurnEvidence {
            scope: scope.clone(),
            model,
            effort,
            speed,
            ts_ms,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cache_write_1h_tokens,
        };
        raw_total_tokens = raw_total_tokens
            .checked_add(turn_total)
            .context("session token total overflowed")?;
        if context > report_context.depth_cap {
            overdepth_avoidable_tokens = overdepth_avoidable_tokens
                .checked_add(
                    avoidable_overdepth_tokens(
                        input,
                        cache_read,
                        cache_write,
                        report_context.depth_cap,
                    )
                    .context("overdepth token calculation overflowed")?,
                )
                .context("overdepth token total overflowed")?;
        }
        // A delegated worker re-reads the same definitions its parent
        // loaded, so its turns replicate the definition too.
        if (scope == "main" || scope == "delegated")
            && let Some(groups) = &mut source_groups
        {
            observe_main_context(groups, context, &turn.model, turn.speed.as_deref())?;
        }
        turn_accumulator.observe(turn);
    }

    let mut result = SessionTokenBurnEvidence::from_session(evidence);
    result.pricing_revision = Some(format!("pricing-generation-{}", pricing_generation()));
    if raw_total_tokens > 0 {
        result.total_tokens = Some(raw_total_tokens);
    }
    (probes.turn)();
    ensure_not_cancelled(cancel)?;
    turn_accumulator.finish_into(&mut result);
    result.overdepth_avoidable_tokens = Some(overdepth_avoidable_tokens);
    if has_unattributed_assistant_turn {
        result.repeated_context_avoidable_tokens = None;
    }
    if let Some([mcp, built_in, skills]) = source_groups {
        result.mcp_sources = finish_source_counters(mcp);
        result.built_in_tool_sources = finish_source_counters(built_in);
        result.skill_sources = finish_source_counters(skills);
    }
    Ok(result)
}

pub(super) fn avoidable_overdepth_tokens(
    input: u128,
    cache_read: u128,
    cache_write: u128,
    depth_cap: u128,
) -> Option<u128> {
    let context = input.checked_add(cache_read)?.checked_add(cache_write)?;
    let excess = context.saturating_sub(depth_cap);
    cache_read
        .checked_add(cache_write)
        .map(|cache| cache.min(excess))
}

fn source_token_counters(
    initial_context: &InitialContextBreakdown,
    source_kind: &str,
    agent: &str,
    project_cwd: Option<&str>,
) -> Option<Vec<SourceTokenCounter>> {
    let matching = initial_context
        .sources
        .iter()
        .filter(|source| source.source == source_kind)
        .collect::<Vec<_>>();
    if matching.iter().any(|source| {
        source.source_name.as_deref().is_none_or(|name| {
            name == "Other skills" || name == "Other MCP servers" || name == "Other built-in tools"
        }) || source.deferred
    }) {
        return None;
    }
    matching
        .into_iter()
        .filter(|source| source.token_count > 0)
        .map(|source| {
            let name = source.source_name.as_deref()?.trim().to_lowercase();
            if name.is_empty() {
                return None;
            }
            let scope = if matches!(source.origin, SourceOrigin::Project | SourceOrigin::Unknown) {
                format!("{agent}:cwd:{}", project_cwd?)
            } else {
                format!("{agent}:{}", source_origin_key(source.origin))
            };
            Some(SourceTokenCounter {
                evidence: TokenBurnSourceEvidence {
                    scope,
                    name,
                    replicated_tokens: 0,
                    invoked: source.use_count > 0,
                    replicated_cost_usd: None,
                },
                definition_tokens: u128::from(source.token_count),
            })
        })
        .collect()
}

/// Adds `context_tokens` to every source whose definition it already
/// carries. Prices the addition at `model`'s cache-read rate when the
/// pricing table resolves it; an unresolvable model still adds tokens,
/// since token and dollar evidence fail independently.
fn observe_main_context(
    groups: &mut [Option<Vec<SourceTokenCounter>>; 3],
    context_tokens: u128,
    model: &str,
    speed: Option<&str>,
) -> Result<()> {
    let canonical_model = canonical_model_key(model);
    let cache_read_cost_per_token = lookup_turn_pricing(model, speed)
        .or_else(|| lookup_turn_pricing(&canonical_model, speed))
        .map(|pricing| pricing.cache_read_cost_per_token);
    for sources in groups.iter_mut().flatten() {
        for source in sources {
            if context_tokens >= source.definition_tokens {
                source.evidence.replicated_tokens = source
                    .evidence
                    .replicated_tokens
                    .checked_add(source.definition_tokens)
                    .context("source replicated token total overflowed")?;
                if let Some(rate) = cache_read_cost_per_token {
                    let contribution = source.definition_tokens as f64 * rate;
                    source.evidence.replicated_cost_usd =
                        Some(source.evidence.replicated_cost_usd.unwrap_or(0.0) + contribution);
                }
            }
        }
    }
    Ok(())
}

fn finish_source_counters(
    counters: Option<Vec<SourceTokenCounter>>,
) -> Option<Vec<TokenBurnSourceEvidence>> {
    counters.map(|counters| {
        counters
            .into_iter()
            .map(|counter| counter.evidence)
            .collect()
    })
}

#[cfg(test)]
pub(super) fn source_token_evidence(
    initial_context: &InitialContextBreakdown,
    source_kind: &str,
    agent: &str,
    skill_cwd: Option<&str>,
    main_context_turns: &[(u128, &str)],
) -> Option<Vec<TokenBurnSourceEvidence>> {
    let mut groups = [
        source_token_counters(initial_context, source_kind, agent, skill_cwd),
        None,
        None,
    ];
    for (capacity, model) in main_context_turns {
        observe_main_context(&mut groups, *capacity, model, None).ok()?;
    }
    finish_source_counters(groups[0].take())
}

fn source_origin_key(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Bundled => "bundled",
        SourceOrigin::Plugin => "plugin",
        SourceOrigin::User => "user",
        SourceOrigin::Project => "project",
        SourceOrigin::Unknown => "unknown",
    }
}
