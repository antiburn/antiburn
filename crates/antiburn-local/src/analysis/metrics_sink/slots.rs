use std::collections::VecDeque;
use std::mem::size_of;

use crate::analysis::model::CompactionTrigger;

use super::tally::NameId;

/// The reorder window sorts records displaced by at most 63 arrivals.
pub(crate) const REORDER_WINDOW: usize = 64;
/// Three slots per chart bucket preserve chart shape within the memory ceiling.
pub(crate) const SLOTS_PER_BUCKET: usize = 3;
/// The slot grid has three times the visible chart resolution.
pub(crate) const SLOTS: usize = crate::analysis::engine::BUCKETS * SLOTS_PER_BUCKET;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StampedName {
    pub(crate) ordinal: u64,
    pub(crate) name: NameId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompactionMark {
    pub(crate) effective_ts: i64,
    pub(crate) ordinal: u64,
    pub(crate) trigger: Option<CompactionTrigger>,
    pub(crate) pre_tokens: Option<u64>,
    pub(crate) post_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CacheSlot {
    pub(crate) is_rehydration: bool,
    pub(crate) is_routing_miss: bool,
    pub(crate) rehydration_gap: Option<(u64, Option<u64>)>,
}

#[derive(Clone, Debug)]
pub(crate) struct SlotAggregate {
    pub(crate) first_ordinal: u64,
    pub(crate) last_ordinal: u64,
    pub(crate) first_key: (i64, u64),
    pub(crate) last_key: (i64, u64),
    pub(crate) first_ts: i64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) tokens_in: u64,
    pub(crate) tokens_out: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) subagent_tokens: u64,
    pub(crate) context_tokens: u64,
    pub(crate) user_prompts: u32,
    pub(crate) subagent_launches: u32,
    pub(crate) has_thinking: bool,
    pub(crate) model: Option<StampedName>,
    pub(crate) thinking_mode: Option<StampedName>,
    pub(crate) speed: Option<StampedName>,
    pub(crate) last_tool: Option<StampedName>,
    pub(crate) first_compaction_key: Option<(i64, u64)>,
    pub(crate) compaction: Option<CompactionMark>,
    pub(crate) first_gap: Option<(u64, u64)>,
    pub(crate) cache_mode_1: CacheSlot,
    pub(crate) cache_mode_2: CacheSlot,
}

impl SlotAggregate {
    pub(crate) fn new(ordinal: u64, effective_ts: i64) -> Self {
        Self {
            first_ordinal: ordinal,
            last_ordinal: ordinal,
            first_key: (effective_ts, ordinal),
            last_key: (effective_ts, ordinal),
            first_ts: effective_ts,
            timestamp: None,
            tokens_in: 0,
            tokens_out: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            subagent_tokens: 0,
            context_tokens: 0,
            user_prompts: 0,
            subagent_launches: 0,
            has_thinking: false,
            model: None,
            thinking_mode: None,
            speed: None,
            last_tool: None,
            first_compaction_key: None,
            compaction: None,
            first_gap: None,
            cache_mode_1: CacheSlot::default(),
            cache_mode_2: CacheSlot::default(),
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.first_ordinal = self.first_ordinal.min(other.first_ordinal);
        self.last_ordinal = self.last_ordinal.max(other.last_ordinal);
        self.first_key = self.first_key.min(other.first_key);
        self.last_key = self.last_key.max(other.last_key);
        self.first_ts = self.first_key.0;
        self.timestamp = match (self.timestamp, other.timestamp) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.tokens_in = self.tokens_in.saturating_add(other.tokens_in);
        self.tokens_out = self.tokens_out.saturating_add(other.tokens_out);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.subagent_tokens = self.subagent_tokens.saturating_add(other.subagent_tokens);
        self.context_tokens = self.context_tokens.max(other.context_tokens);
        self.user_prompts = self.user_prompts.saturating_add(other.user_prompts);
        self.subagent_launches = self
            .subagent_launches
            .saturating_add(other.subagent_launches);
        self.has_thinking |= other.has_thinking;
        merge_last(&mut self.model, other.model);
        merge_last(&mut self.thinking_mode, other.thinking_mode);
        merge_last(&mut self.speed, other.speed);
        merge_last(&mut self.last_tool, other.last_tool);
        self.first_compaction_key = match (self.first_compaction_key, other.first_compaction_key) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        merge_compaction(&mut self.compaction, other.compaction);
        merge_first_gap(&mut self.first_gap, other.first_gap);
        merge_cache(&mut self.cache_mode_1, other.cache_mode_1);
        merge_cache(&mut self.cache_mode_2, other.cache_mode_2);
    }
}

fn merge_last(target: &mut Option<StampedName>, incoming: Option<StampedName>) {
    if incoming.is_some_and(|value| target.is_none_or(|current| value.ordinal > current.ordinal)) {
        *target = incoming;
    }
}

fn merge_compaction(target: &mut Option<CompactionMark>, incoming: Option<CompactionMark>) {
    if incoming.is_some_and(|value| target.is_none_or(|current| value.ordinal > current.ordinal)) {
        *target = incoming;
    }
}

fn merge_first_gap(target: &mut Option<(u64, u64)>, incoming: Option<(u64, u64)>) {
    if incoming.is_some_and(|value| target.is_none_or(|current| value.0 < current.0)) {
        *target = incoming;
    }
}

fn merge_cache(target: &mut CacheSlot, incoming: CacheSlot) {
    target.is_rehydration |= incoming.is_rehydration;
    target.is_routing_miss |= incoming.is_routing_miss;
    if incoming.rehydration_gap.is_some_and(|value| {
        target
            .rehydration_gap
            .is_none_or(|current| value.0 > current.0)
    }) {
        target.rehydration_gap = incoming.rehydration_gap;
    }
}

#[derive(Clone)]
pub(crate) struct ReorderWindow {
    entries: VecDeque<SlotAggregate>,
    ordered: bool,
    last_popped: Option<(i64, u64)>,
    pub(crate) overflow: u64,
}

impl Default for ReorderWindow {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            ordered: true,
            last_popped: None,
            overflow: 0,
        }
    }
}

impl ReorderWindow {
    pub(crate) fn push(&mut self, mut slot: SlotAggregate) -> Option<SlotAggregate> {
        let ready = (self.entries.len() == REORDER_WINDOW).then(|| self.pop_minimum());
        let key = (slot.first_ts, slot.first_ordinal);
        if self.last_popped.is_some_and(|last| key < last) {
            let timestamp = self.last_popped.map_or(slot.first_ts, |last| last.0);
            clamp_timestamp(&mut slot, timestamp);
            self.overflow = self.overflow.saturating_add(1);
            tracing::debug!(event = "metrics_reorder_window_capped");
        }
        if self
            .entries
            .back()
            .is_some_and(|last| key < (last.first_ts, last.first_ordinal))
        {
            self.ordered = false;
        }
        self.entries.push_back(slot);
        ready
    }

    fn pop_minimum(&mut self) -> SlotAggregate {
        let mut slot = if self.ordered {
            self.entries.pop_front().unwrap_or_else(|| unreachable!())
        } else {
            let index = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, slot)| (slot.first_ts, slot.first_ordinal))
                .map(|(index, _)| index)
                .unwrap_or(0);
            let slot = self.entries.remove(index).unwrap_or_else(|| unreachable!());
            self.ordered =
                self.entries
                    .iter()
                    .zip(self.entries.iter().skip(1))
                    .all(|(left, right)| {
                        (left.first_ts, left.first_ordinal) <= (right.first_ts, right.first_ordinal)
                    });
            slot
        };
        let key = (slot.first_ts, slot.first_ordinal);
        if self.last_popped.is_some_and(|last| key < last) {
            let timestamp = self.last_popped.map_or(slot.first_ts, |last| last.0);
            clamp_timestamp(&mut slot, timestamp);
            self.overflow = self.overflow.saturating_add(1);
            tracing::debug!(event = "metrics_reorder_window_capped");
        }
        self.last_popped = Some((slot.first_ts, slot.first_ordinal));
        slot
    }

    pub(crate) fn drain_sorted(&mut self) -> Vec<SlotAggregate> {
        let mut drained = Vec::with_capacity(self.entries.len());
        while !self.entries.is_empty() {
            drained.push(self.pop_minimum());
        }
        self.entries.shrink_to_fit();
        self.ordered = true;
        drained
    }

    pub(crate) fn merge_cache_patch(&mut self, ordinal: u64, patch: CacheSlot) -> bool {
        let Some(slot) = self
            .entries
            .iter_mut()
            .rev()
            .find(|slot| slot.first_ordinal == ordinal)
        else {
            return false;
        };
        merge_cache(&mut slot.cache_mode_2, patch);
        true
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &SlotAggregate> {
        self.entries.iter()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.entries
            .capacity()
            .saturating_mul(size_of::<SlotAggregate>())
    }
}

fn clamp_timestamp(slot: &mut SlotAggregate, timestamp: i64) {
    slot.first_ts = timestamp;
    slot.first_key.0 = timestamp;
    slot.last_key.0 = timestamp;
    if let Some(key) = &mut slot.first_compaction_key {
        key.0 = timestamp;
    }
    if let Some(mark) = &mut slot.compaction {
        mark.effective_ts = timestamp;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SlotAxis {
    #[default]
    Ordinal,
    Active,
}

#[derive(Clone, Debug)]
struct PositionedSlot {
    index: u64,
    aggregate: SlotAggregate,
}

#[derive(Clone, Default)]
pub(crate) struct ProgressSlots {
    slots: Vec<PositionedSlot>,
    quantum: u64,
    axis: SlotAxis,
    pub(crate) compactions: u32,
}

impl ProgressSlots {
    pub(crate) fn axis(&self) -> SlotAxis {
        self.axis
    }

    pub(crate) fn flip_to_active(&mut self, position: impl Fn(i64) -> u64) {
        if self.axis == SlotAxis::Active {
            return;
        }
        let aggregates = std::mem::take(&mut self.slots)
            .into_iter()
            .map(|slot| slot.aggregate)
            .collect::<Vec<_>>();
        self.quantum = 1;
        self.axis = SlotAxis::Active;
        for aggregate in aggregates {
            let active_position = position(aggregate.first_ts);
            self.push(aggregate, active_position);
        }
    }

    pub(crate) fn push(&mut self, slot: SlotAggregate, position: u64) {
        if self.quantum == 0 {
            self.quantum = 1;
        }
        loop {
            let index = position / self.quantum;
            match self.slots.binary_search_by_key(&index, |slot| slot.index) {
                Ok(existing) => {
                    self.slots[existing].aggregate.merge(slot);
                    return;
                }
                Err(insert_at) if self.slots.len() < SLOTS => {
                    self.reserve_one();
                    self.slots.insert(
                        insert_at,
                        PositionedSlot {
                            index,
                            aggregate: slot,
                        },
                    );
                    return;
                }
                Err(_) => self.double_quantum(),
            }
        }
    }

    fn reserve_one(&mut self) {
        if self.slots.len() < self.slots.capacity() {
            return;
        }
        let current = self.slots.capacity();
        let target = current.saturating_add(8).clamp(8, SLOTS);
        if target > current {
            self.slots.reserve_exact(target - current);
        }
    }

    fn double_quantum(&mut self) {
        let old = std::mem::take(&mut self.slots);
        let capacity = old.len().div_ceil(2).min(SLOTS);
        let mut compacted: Vec<PositionedSlot> = Vec::with_capacity(capacity);
        for mut slot in old {
            slot.index /= 2;
            if let Some(last) = compacted.last_mut()
                && last.index == slot.index
            {
                last.aggregate.merge(slot.aggregate);
            } else {
                compacted.push(slot);
            }
        }
        self.slots = compacted;
        self.quantum = self.quantum.saturating_mul(2).max(1);
        self.compactions = self.compactions.saturating_add(1);
        tracing::debug!(event = "metrics_progress_slots_compacted");
    }

    pub(crate) fn merge_cache_patch(&mut self, key: (i64, u64), patch: CacheSlot) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| key >= slot.aggregate.first_key && key <= slot.aggregate.last_key)
        else {
            return false;
        };
        merge_cache(&mut slot.aggregate.cache_mode_2, patch);
        true
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &SlotAggregate> {
        self.slots.iter().map(|slot| &slot.aggregate)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.slots
            .capacity()
            .saturating_mul(size_of::<PositionedSlot>())
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_wins_uses_the_arrival_ordinal() {
        let mut later = SlotAggregate::new(8, 8);
        later.model = Some(StampedName {
            ordinal: 8,
            name: NameId(2),
        });
        let mut earlier = SlotAggregate::new(3, 3);
        earlier.model = Some(StampedName {
            ordinal: 3,
            name: NameId(1),
        });
        later.merge(earlier);
        assert_eq!(later.model.expect("model").name, NameId(2));
    }

    #[test]
    fn compaction_metadata_stays_atomic() {
        let mut first = SlotAggregate::new(1, 1);
        first.compaction = Some(CompactionMark {
            effective_ts: 1,
            ordinal: 1,
            trigger: Some(CompactionTrigger::Manual),
            pre_tokens: Some(100),
            post_tokens: Some(50),
        });
        let mut second = SlotAggregate::new(2, 2);
        second.compaction = Some(CompactionMark {
            effective_ts: 2,
            ordinal: 2,
            trigger: None,
            pre_tokens: None,
            post_tokens: None,
        });
        first.merge(second);
        assert_eq!(first.compaction.expect("compaction").trigger, None);
        assert_eq!(first.compaction.expect("compaction").pre_tokens, None);
    }

    #[test]
    fn merged_slot_uses_the_earliest_timestamp_key() {
        let mut later_ordinal = SlotAggregate::new(900, 10);
        let earlier_ordinal = SlotAggregate::new(5, 20);
        later_ordinal.merge(earlier_ordinal);
        assert_eq!(later_ordinal.first_ts, 10);
        assert_eq!(later_ordinal.first_key, (10, 900));
    }

    #[test]
    fn cache_patch_uses_timestamp_key_not_an_ordinal_range() {
        let mut wide = SlotAggregate::new(5, 10);
        wide.merge(SlotAggregate::new(900, 20));
        let mut narrow = SlotAggregate::new(50, 30);
        narrow.merge(SlotAggregate::new(60, 40));
        let mut slots = ProgressSlots::default();
        slots.push(wide, 10);
        slots.push(narrow, 30);
        assert!(slots.merge_cache_patch(
            (30, 50),
            CacheSlot {
                is_rehydration: true,
                ..CacheSlot::default()
            }
        ));
        assert!(!slots.slots[0].aggregate.cache_mode_2.is_rehydration);
        assert!(slots.slots[1].aggregate.cache_mode_2.is_rehydration);
        assert!(slots.merge_cache_patch(
            (30, 50),
            CacheSlot {
                is_routing_miss: true,
                ..CacheSlot::default()
            }
        ));
        assert!(slots.slots[1].aggregate.cache_mode_2.is_rehydration);
        assert!(slots.slots[1].aggregate.cache_mode_2.is_routing_miss);
    }

    #[test]
    fn sparse_positions_do_not_compact_before_the_grid_is_full() {
        let mut slots = ProgressSlots::default();
        for ordinal in 0..24 {
            slots.push(
                SlotAggregate::new(ordinal, ordinal as i64),
                ordinal.saturating_mul(300_000),
            );
        }
        assert_eq!(slots.len(), 24);
        assert_eq!(slots.quantum, 1);
        assert_eq!(slots.compactions, 0);
    }

    #[test]
    fn storage_stays_bounded_after_many_contributions() {
        let mut slots = ProgressSlots::default();
        for ordinal in 0..100_000 {
            slots.push(SlotAggregate::new(ordinal, ordinal as i64), ordinal);
        }
        assert!(slots.len() <= SLOTS);
        assert!(slots.compactions > 0);
    }

    #[test]
    fn reorder_clamp_moves_compaction_keys_with_the_slot() {
        let mut window = ReorderWindow::default();
        for timestamp in 1..=REORDER_WINDOW {
            assert!(
                window
                    .push(SlotAggregate::new(timestamp as u64, timestamp as i64))
                    .is_none()
            );
        }
        let mut compaction = SlotAggregate::new(0, 0);
        compaction.first_compaction_key = Some((0, 0));
        compaction.compaction = Some(CompactionMark {
            effective_ts: 0,
            ordinal: 0,
            trigger: None,
            pre_tokens: None,
            post_tokens: None,
        });
        window.push(compaction);
        let clamped = window
            .iter()
            .find(|slot| slot.first_ordinal == 0)
            .expect("the compaction remains in the window");
        assert_eq!(clamped.first_ts, 1);
        assert_eq!(clamped.first_compaction_key, Some((1, 0)));
        assert_eq!(clamped.compaction.expect("compaction").effective_ts, 1);
    }
}
