use std::collections::VecDeque;
use std::mem::size_of;

use super::slots::CacheSlot;
use super::tally::IdentityKey;

pub(crate) const CACHE_REHYDRATION_MIN_CONTEXT_TOKENS: u64 = 20_000;
pub(crate) const CACHE_REHYDRATION_MIN_GAP_SECS: u64 = 60;
/// Model deferral spans only the transition to the first explicit model.
pub(crate) const MAX_DEFERRED_CACHE: usize = 8;
const CACHE_REHYDRATION_WRITE_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_PRIOR_READ_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_MISS_READ_RATIO: f64 = 0.2;
const CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO: f64 = 0.8;
const CACHE_REHYDRATION_REPLAY_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_RECOVERY_READ_RATIO: f64 = 0.5;

#[derive(Clone, Copy)]
pub(crate) struct CacheTurn {
    key: (i64, u64),
    context_tokens: u64,
    fresh_input_tokens: u64,
    cache_read_tokens: u64,
    first_turn_after_compaction: bool,
    secs_since_prior_turn: Option<u64>,
    model: Option<IdentityKey>,
}

pub(crate) struct CacheInput {
    pub(crate) key: (i64, u64),
    pub(crate) timestamp: Option<i64>,
    pub(crate) context_tokens: u64,
    pub(crate) fresh_input_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) model: Option<IdentityKey>,
}

#[derive(Clone, Copy)]
struct DeferredCache {
    key: (i64, u64),
    gap: Option<u64>,
    previous_model: Option<IdentityKey>,
    current_model: Option<IdentityKey>,
    next_model: Option<IdentityKey>,
}

pub(crate) struct CachePatch {
    pub(crate) key: (i64, u64),
    pub(crate) slot: CacheSlot,
}

#[derive(Clone, Default)]
pub(crate) struct CacheReducer {
    previous_context: u64,
    previous_cache_read: u64,
    first_turn_after_compaction: bool,
    previous_turn_ts: Option<i64>,
    turns: VecDeque<CacheTurn>,
    deferred: Vec<DeferredCache>,
    pub(crate) mode_1_rehydrations: u64,
    pub(crate) mode_1_routing_misses: u64,
    pub(crate) mode_2_rehydrations: u64,
    pub(crate) mode_2_routing_misses: u64,
}

impl CacheReducer {
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
            fresh_input_tokens: input.fresh_input_tokens,
            cache_read_tokens: input.cache_read_tokens,
            first_turn_after_compaction: self.first_turn_after_compaction,
            secs_since_prior_turn: gap,
            model: input.model,
        };
        let mut mode_1 = CacheSlot::default();
        if is_cache_rehydration_turn(
            input.context_tokens,
            input.cache_write_tokens,
            self.previous_context,
            self.previous_cache_read,
            self.first_turn_after_compaction,
        ) {
            classify(&mut mode_1, input.key.1, gap);
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
        self.previous_context = input.context_tokens;
        self.previous_cache_read = input.cache_read_tokens;
        self.first_turn_after_compaction = false;
        self.previous_turn_ts = input.timestamp;
        (mode_1, mode_2, gap)
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
                });
            } else {
                tracing::debug!(event = "metrics_cache_deferral_capped");
            }
            return None;
        }
        if !models_match(previous.model, current.model, next.model, None) {
            return None;
        }
        Some(self.mode_2_patch(current.key, current.secs_since_prior_turn))
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
            .map(|candidate| self.mode_2_patch(candidate.key, candidate.gap))
            .collect()
    }

    fn mode_2_patch(&mut self, key: (i64, u64), gap: Option<u64>) -> CachePatch {
        let mut slot = CacheSlot::default();
        classify(&mut slot, key.1, gap);
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

fn classify(slot: &mut CacheSlot, ordinal: u64, gap: Option<u64>) {
    if gap_allows_rehydration(gap) {
        slot.is_rehydration = true;
        slot.rehydration_gap = Some((ordinal, gap));
    } else {
        slot.is_routing_miss = true;
    }
}

pub(crate) fn gap_allows_rehydration(gap: Option<u64>) -> bool {
    gap.is_none_or(|seconds| seconds >= CACHE_REHYDRATION_MIN_GAP_SECS)
}

fn cache_ratio(tokens: u64, context_tokens: u64) -> f64 {
    if context_tokens == 0 {
        0.0
    } else {
        tokens as f64 / context_tokens as f64
    }
}

fn same_known_model(left: Option<IdentityKey>, right: Option<IdentityKey>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn is_cache_rehydration_turn(
    context_tokens: u64,
    cache_write_tokens: u64,
    previous_context_tokens: u64,
    previous_cache_read_tokens: u64,
    first_turn_after_compaction: bool,
) -> bool {
    if first_turn_after_compaction
        || context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || previous_context_tokens == 0
    {
        return false;
    }
    cache_ratio(cache_write_tokens, context_tokens) >= CACHE_REHYDRATION_WRITE_RATIO
        && cache_ratio(previous_cache_read_tokens, previous_context_tokens)
            >= CACHE_REHYDRATION_PRIOR_READ_RATIO
}

fn inferred_cache_rehydration_turn(previous: CacheTurn, current: CacheTurn) -> bool {
    if current.first_turn_after_compaction
        || current.context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || cache_ratio(previous.cache_read_tokens, previous.context_tokens)
            < CACHE_REHYDRATION_PRIOR_READ_RATIO
        || cache_ratio(current.cache_read_tokens, current.context_tokens)
            > CACHE_REHYDRATION_MISS_READ_RATIO
    {
        return false;
    }
    if current.context_tokens as f64 / (previous.context_tokens as f64)
        < CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO
    {
        return false;
    }
    let growth = current
        .context_tokens
        .saturating_sub(previous.context_tokens);
    let replayed = current.fresh_input_tokens.saturating_sub(growth);
    cache_ratio(replayed, current.context_tokens) >= CACHE_REHYDRATION_REPLAY_RATIO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_gap_allows_rehydration() {
        assert!(gap_allows_rehydration(None));
    }

    #[test]
    fn reorder_clamp_updates_pending_cache_keys() {
        let mut reducer = CacheReducer::default();
        reducer.observe(CacheInput {
            key: (0, 0),
            timestamp: Some(0),
            context_tokens: 30_000,
            fresh_input_tokens: 1_000,
            cache_read_tokens: 29_000,
            cache_write_tokens: 0,
            model: None,
        });
        reducer.update_key(0, (5, 0));
        assert_eq!(reducer.turns[0].key, (5, 0));
    }

    #[test]
    fn a_fast_gap_is_a_routing_miss() {
        let mut slot = CacheSlot::default();
        classify(&mut slot, 4, Some(10));
        assert!(slot.is_routing_miss);
        assert!(!slot.is_rehydration);
    }
}
