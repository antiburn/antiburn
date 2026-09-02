use std::collections::VecDeque;
use std::mem::size_of;

use serde::{Deserialize, Serialize};

use super::slots::{CacheRehydrationMark, CacheSlot};
use super::tally::IdentityKey;

pub(crate) const CACHE_REHYDRATION_MIN_CONTEXT_TOKENS: u64 = 20_000;
/// Model deferral spans only the transition to the first explicit model.
pub(crate) const MAX_DEFERRED_CACHE: usize = 8;
const CACHE_REHYDRATION_PRIOR_READ_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO: f64 = 0.8;
const CACHE_REHYDRATION_RECOVERY_READ_RATIO: f64 = 0.5;
const CLAUDE_REHYDRATION_MIN_USER_INACTIVITY_SECS: u64 = 60 * 60;
const CODEX_REHYDRATION_MIN_USER_INACTIVITY_SECS: u64 = 30 * 60;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct CacheTurn {
    key: (i64, u64),
    context_tokens: u64,
    cache_read_tokens: u64,
    first_turn_after_compaction: bool,
    secs_since_prior_turn: Option<u64>,
    user_inactive_secs: Option<u64>,
    model: Option<IdentityKey>,
}

pub(crate) struct CacheInput {
    pub(crate) key: (i64, u64),
    pub(crate) timestamp: Option<i64>,
    pub(crate) context_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) model: Option<IdentityKey>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct DeferredCache {
    key: (i64, u64),
    gap: Option<u64>,
    previous_model: Option<IdentityKey>,
    current_model: Option<IdentityKey>,
    next_model: Option<IdentityKey>,
    rehydration: CacheRehydrationMark,
}

pub(crate) struct CachePatch {
    pub(crate) key: (i64, u64),
    pub(crate) slot: CacheSlot,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub(crate) struct CacheReducer {
    rehydration_min_user_inactivity_secs: Option<u64>,
    has_explicit_cache_writes: bool,
    first_turn_after_compaction: bool,
    previous_turn_ts: Option<i64>,
    pending_user_prompt: bool,
    pending_user_prompt_ts: Option<i64>,
    turns: VecDeque<CacheTurn>,
    deferred: Vec<DeferredCache>,
    pub(crate) mode_1_rehydrations: u64,
    pub(crate) mode_1_routing_misses: u64,
    pub(crate) mode_2_rehydrations: u64,
    pub(crate) mode_2_routing_misses: u64,
}

impl CacheReducer {
    pub(crate) fn new(agent: &str) -> Self {
        Self {
            rehydration_min_user_inactivity_secs: rehydration_min_user_inactivity_secs(agent),
            ..Self::default()
        }
    }

    pub(crate) fn update_key(&mut self, ordinal: u64, key: (i64, u64)) {
        for turn in &mut self.turns {
            if turn.key.1 == ordinal {
                turn.key = key;
            }
        }
        for candidate in &mut self.deferred {
            if candidate.key.1 == ordinal {
                candidate.key = key;
            }
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.turns
            .capacity()
            .saturating_mul(size_of::<CacheTurn>())
            .saturating_add(
                self.deferred
                    .capacity()
                    .saturating_mul(size_of::<DeferredCache>()),
            )
    }

    pub(crate) fn mark_compaction(&mut self) {
        self.first_turn_after_compaction = true;
    }

    pub(crate) fn observe_user_prompt(&mut self, timestamp: Option<i64>) {
        self.pending_user_prompt = true;
        if let Some(timestamp) = timestamp {
            self.pending_user_prompt_ts.get_or_insert(timestamp);
        }
    }

    pub(crate) fn has_explicit_cache_writes(&self) -> bool {
        self.has_explicit_cache_writes
    }

    pub(crate) fn observe(
        &mut self,
        input: CacheInput,
    ) -> (CacheSlot, Option<CachePatch>, Option<u64>) {
        let gap = input
            .timestamp
            .zip(self.previous_turn_ts)
            .map(|(current, prior)| u64::try_from((current - prior).max(0) / 1_000).unwrap_or(0));
        let turn = CacheTurn {
            key: input.key,
            context_tokens: input.context_tokens,
            cache_read_tokens: input.cache_read_tokens,
            first_turn_after_compaction: self.first_turn_after_compaction,
            secs_since_prior_turn: gap,
            user_inactive_secs: self.user_inactivity_secs(),
            model: input.model,
        };
        self.pending_user_prompt = false;
        self.pending_user_prompt_ts = None;
        let mut mode_1 = CacheSlot::default();
        self.has_explicit_cache_writes |= input.cache_write_tokens > 0;
        if self.turns.back().is_some_and(|previous| {
            is_direct_cache_event(*previous, turn, input.cache_write_tokens)
        }) {
            let previous = *self.turns.back().expect("the prior cache turn");
            classify(
                &mut mode_1,
                rehydration_mark(previous.context_tokens, turn),
                gap,
                self.rehydration_min_user_inactivity_secs,
            );
            count_slot(
                mode_1,
                &mut self.mode_1_rehydrations,
                &mut self.mode_1_routing_misses,
            );
        }
        self.turns.push_back(turn);
        let mode_2 = if self.turns.len() == 3 {
            let previous = self.turns[0];
            let current = self.turns[1];
            let next = self.turns[2];
            self.turns.pop_front();
            self.resolve_window(previous, current, next)
        } else {
            None
        };
        self.first_turn_after_compaction = false;
        self.previous_turn_ts = input.timestamp;
        (mode_1, mode_2, gap)
    }

    fn user_inactivity_secs(&self) -> Option<u64> {
        if !self.pending_user_prompt {
            return None;
        }
        self.pending_user_prompt_ts
            .zip(self.previous_turn_ts)
            .map(|(prompt, prior)| u64::try_from((prompt - prior).max(0) / 1_000).unwrap_or(0))
    }

    fn resolve_window(
        &mut self,
        previous: CacheTurn,
        current: CacheTurn,
        next: CacheTurn,
    ) -> Option<CachePatch> {
        if !inferred_cache_rehydration_turn(previous, current)
            || next.first_turn_after_compaction
            || cache_ratio(next.cache_read_tokens, next.context_tokens)
                < CACHE_REHYDRATION_RECOVERY_READ_RATIO
            || next.context_tokens as f64 / (current.context_tokens as f64)
                < CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO
        {
            return None;
        }
        let models = [previous.model, current.model, next.model];
        let rehydration = rehydration_mark(previous.context_tokens, current);
        let has_missing = models.iter().any(Option::is_none);
        let has_explicit = models.iter().any(Option::is_some);
        if has_missing && has_explicit {
            if self.deferred.len() < MAX_DEFERRED_CACHE {
                self.deferred.push(DeferredCache {
                    key: current.key,
                    gap: current.secs_since_prior_turn,
                    previous_model: previous.model,
                    current_model: current.model,
                    next_model: next.model,
                    rehydration,
                });
            } else {
                tracing::debug!(event = "metrics_cache_deferral_capped");
            }
            return None;
        }
        if !models_match(previous.model, current.model, next.model, None) {
            return None;
        }
        Some(self.mode_2_patch(current.key, current.secs_since_prior_turn, rehydration))
    }

    pub(crate) fn resolve_deferred(&mut self, fallback: Option<IdentityKey>) -> Vec<CachePatch> {
        let deferred = std::mem::take(&mut self.deferred);
        deferred
            .into_iter()
            .filter(|candidate| {
                models_match(
                    candidate.previous_model,
                    candidate.current_model,
                    candidate.next_model,
                    fallback,
                )
            })
            .map(|candidate| self.mode_2_patch(candidate.key, candidate.gap, candidate.rehydration))
            .collect()
    }

    fn mode_2_patch(
        &mut self,
        key: (i64, u64),
        gap: Option<u64>,
        rehydration: CacheRehydrationMark,
    ) -> CachePatch {
        let mut slot = CacheSlot::default();
        classify(
            &mut slot,
            rehydration,
            gap,
            self.rehydration_min_user_inactivity_secs,
        );
        count_slot(
            slot,
            &mut self.mode_2_rehydrations,
            &mut self.mode_2_routing_misses,
        );
        CachePatch { key, slot }
    }
}

fn models_match(
    previous: Option<IdentityKey>,
    current: Option<IdentityKey>,
    next: Option<IdentityKey>,
    fallback: Option<IdentityKey>,
) -> bool {
    same_known_model(previous.or(fallback), current.or(fallback))
        && same_known_model(current.or(fallback), next.or(fallback))
}

fn count_slot(slot: CacheSlot, rehydrations: &mut u64, routing_misses: &mut u64) {
    if slot.is_rehydration {
        *rehydrations = rehydrations.saturating_add(1);
    } else if slot.is_routing_miss {
        *routing_misses = routing_misses.saturating_add(1);
    }
}

fn classify(
    slot: &mut CacheSlot,
    rehydration: CacheRehydrationMark,
    gap: Option<u64>,
    rehydration_min_user_inactivity_secs: Option<u64>,
) {
    slot.is_rehydration = rehydration_min_user_inactivity_secs.is_some_and(|minimum| {
        rehydration
            .user_inactive_secs
            .is_some_and(|inactivity| inactivity >= minimum)
    });
    slot.is_routing_miss = !slot.is_rehydration;
    slot.rehydration_gap = Some((rehydration.ordinal, gap));
    slot.rehydration = Some(rehydration);
}

fn rehydration_mark(previous_context: u64, current: CacheTurn) -> CacheRehydrationMark {
    let still_cached_tokens = current.cache_read_tokens.min(current.context_tokens);
    let uncached_tokens = current.context_tokens.saturating_sub(still_cached_tokens);
    let growth_tokens = current
        .context_tokens
        .saturating_sub(previous_context)
        .min(uncached_tokens);
    CacheRehydrationMark {
        ordinal: current.key.1,
        context_tokens: current.context_tokens,
        still_cached_tokens,
        rewritten_tokens: uncached_tokens.saturating_sub(growth_tokens),
        growth_tokens,
        user_inactive_secs: current.user_inactive_secs,
    }
}

fn cache_ratio(tokens: u64, context_tokens: u64) -> f64 {
    if context_tokens == 0 {
        0.0
    } else {
        tokens as f64 / context_tokens as f64
    }
}

fn rehydration_min_user_inactivity_secs(agent: &str) -> Option<u64> {
    match agent {
        "claude" => Some(CLAUDE_REHYDRATION_MIN_USER_INACTIVITY_SECS),
        "codex" => Some(CODEX_REHYDRATION_MIN_USER_INACTIVITY_SECS),
        _ => None,
    }
}

fn same_known_model(left: Option<IdentityKey>, right: Option<IdentityKey>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn is_direct_cache_event(previous: CacheTurn, current: CacheTurn, cache_write_tokens: u64) -> bool {
    cache_write_tokens >= CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        && is_material_cache_event(previous, current)
}

fn inferred_cache_rehydration_turn(previous: CacheTurn, current: CacheTurn) -> bool {
    is_material_cache_event(previous, current)
}

fn is_material_cache_event(previous: CacheTurn, current: CacheTurn) -> bool {
    if current.first_turn_after_compaction
        || current.context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || cache_ratio(previous.cache_read_tokens, previous.context_tokens)
            < CACHE_REHYDRATION_PRIOR_READ_RATIO
        || !same_known_model(previous.model, current.model)
    {
        return false;
    }
    if current.context_tokens as f64 / (previous.context_tokens as f64)
        < CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO
    {
        return false;
    }
    rehydration_mark(previous.context_tokens, current).rewritten_tokens
        >= CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_clamp_updates_pending_cache_keys() {
        let mut reducer = CacheReducer::default();
        reducer.observe(CacheInput {
            key: (0, 0),
            timestamp: Some(0),
            context_tokens: 30_000,
            cache_read_tokens: 29_000,
            cache_write_tokens: 0,
            model: None,
        });
        reducer.update_key(0, (5, 0));
        assert_eq!(reducer.turns[0].key, (5, 0));
    }

    #[test]
    fn a_fast_gap_is_a_provider_cache_miss() {
        let mut slot = CacheSlot::default();
        classify(
            &mut slot,
            CacheRehydrationMark {
                ordinal: 4,
                context_tokens: 30_000,
                still_cached_tokens: 5_000,
                rewritten_tokens: 25_000,
                growth_tokens: 0,
                user_inactive_secs: Some(10),
            },
            Some(10),
            Some(CLAUDE_REHYDRATION_MIN_USER_INACTIVITY_SECS),
        );
        assert!(slot.is_routing_miss);
        assert!(!slot.is_rehydration);
        assert_eq!(slot.rehydration_gap, Some((4, Some(10))));
        assert!(slot.rehydration.is_some());
    }

    #[test]
    fn meaningful_inactivity_thresholds_are_provider_specific() {
        assert_eq!(rehydration_min_user_inactivity_secs("claude"), Some(3_600));
        assert_eq!(rehydration_min_user_inactivity_secs("codex"), Some(1_800));
        assert_eq!(rehydration_min_user_inactivity_secs("synthetic"), None);
    }
}
