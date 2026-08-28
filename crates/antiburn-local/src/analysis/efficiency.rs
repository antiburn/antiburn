//! Spend efficiency for one thread of turns.
//!
//! Every dollar a turn costs goes to one of three places. New work is output
//! plus fresh input that grows context. Carry is cache-read spend. Rewrite is
//! fresh input that does not grow context.
//!
//! A parent transcript and each subagent transcript are separate threads.
//! Callers add their totals instead of combining their context sequences.
//! The reducer uses usage counters and pricing data. It does not read transcript text.
//! An unpriced turn increments `unpriced_turns` and no cost field.

use std::collections::VecDeque;
use std::mem::size_of;

use serde::{Deserialize, Serialize};

use crate::analysis::model::{NormalizedEvent, Role, Usage};
use crate::analysis::pricing::{lookup_pricing, strip_window_tag};
use crate::pricing::ModelPricing;

/// Open message state covers heavily interleaved parent and sidechain records.
const MAX_OPEN_MESSAGES: usize = 64;
/// The finalized-turn window restores local timestamp order.
const MAX_EFF_REORDER: usize = 32;
/// The contribution list keeps eight entries per visible chart bucket.
const MAX_EFF_CONTRIBUTIONS: usize = 1_440;

/// Additive spend totals for one or more threads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EfficiencyTotals {
    /// The total cost of priced turns.
    pub total_usd: f64,
    /// Output and fresh input that grows context.
    pub new_work_usd: f64,
    /// Cache-read spend.
    pub carry_usd: f64,
    /// Fresh input that does not grow context.
    pub rewrite_usd: f64,
    /// Context growth from priced turns.
    pub growth_tokens: u64,
    /// Output from priced turns.
    pub output_tokens: u64,
    /// Turns with known pricing.
    pub priced_turns: u64,
    /// Output turns without known pricing.
    pub unpriced_turns: u64,
}

impl EfficiencyTotals {
    /// Adds another thread without combining thread context.
    pub fn add(&mut self, other: EfficiencyTotals) {
        self.total_usd += other.total_usd;
        self.new_work_usd += other.new_work_usd;
        self.carry_usd += other.carry_usd;
        self.rewrite_usd += other.rewrite_usd;
        self.growth_tokens = self.growth_tokens.saturating_add(other.growth_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.priced_turns = self.priced_turns.saturating_add(other.priced_turns);
        self.unpriced_turns = self.unpriced_turns.saturating_add(other.unpriced_turns);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MessageKey {
    first: u64,
    second: u64,
    length: usize,
}

impl MessageKey {
    fn new(value: &str) -> Self {
        Self {
            first: hash_bytes(value.as_bytes(), 0xcbf2_9ce4_8422_2325),
            second: hash_bytes(value.as_bytes(), 0x8422_2325_cbf2_9ce4),
            length: value.len(),
        }
    }
}

fn hash_bytes(bytes: &[u8], seed: u64) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        hash.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(*byte)
    })
}

#[derive(Clone)]
enum ModelStatus {
    Missing,
    Priced(ModelPricing),
    Unpriced,
}

#[derive(Clone)]
struct Turn {
    creation: u64,
    ts: i64,
    id: Option<MessageKey>,
    model: ModelStatus,
    usage: Usage,
}

#[derive(Clone, Copy)]
pub(crate) struct EfficiencyInput<'a> {
    pub(crate) ts_ms: Option<i64>,
    pub(crate) role: Role,
    pub(crate) message_id: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) usage: Usage,
}

#[derive(Clone, Copy)]
enum Contribution {
    Priced {
        new_work: f64,
        carry: f64,
        rewrite: f64,
        growth: u64,
        output: u64,
    },
    Fallback {
        usage: Usage,
        growth: u64,
    },
    Unpriced,
}

#[derive(Clone, Copy, Default)]
struct FallbackOverflow {
    growth_tokens: u64,
    output_tokens: u64,
    new_input_tokens: f64,
    new_cache_tokens: f64,
    rewrite_input_tokens: f64,
    rewrite_cache_tokens: f64,
    cache_read_tokens: u64,
    turns: u64,
}

#[derive(Clone)]
pub(crate) struct EfficiencyReducer {
    open: VecDeque<Turn>,
    reorder: Vec<Turn>,
    contributions: Vec<Contribution>,
    overflow_totals: EfficiencyTotals,
    fallback_overflow: FallbackOverflow,
    previous_context: Option<u64>,
    last_ts: i64,
    creation: u64,
    pub(crate) open_overflow: u64,
    pub(crate) reorder_overflow: u64,
    pub(crate) contribution_overflow: u64,
}

impl Default for EfficiencyReducer {
    fn default() -> Self {
        Self {
            open: VecDeque::new(),
            reorder: Vec::new(),
            contributions: Vec::new(),
            overflow_totals: EfficiencyTotals::default(),
            fallback_overflow: FallbackOverflow::default(),
            previous_context: None,
            last_ts: i64::MIN,
            creation: 0,
            open_overflow: 0,
            reorder_overflow: 0,
            contribution_overflow: 0,
        }
    }
}

impl EfficiencyReducer {
    pub(crate) fn observe(&mut self, input: EfficiencyInput<'_>) {
        if let Some(timestamp) = input.ts_ms {
            self.last_ts = timestamp;
        }
        if input.role != Role::Assistant {
            return;
        }
        let message_key = input.message_id.map(MessageKey::new);
        if let Some(index) = message_key.and_then(|id| {
            self.open
                .iter()
                .position(|turn| turn.id.as_ref() == Some(&id))
        }) {
            let turn = &mut self.open[index];
            turn.usage = turn.usage.saturating_add(input.usage);
            if matches!(turn.model, ModelStatus::Missing) {
                turn.model = model_status(input.model);
            }
            return;
        }
        if self.open.len() == MAX_OPEN_MESSAGES
            && let Some(turn) = self.open.pop_front()
        {
            self.finalize_turn(turn);
            self.open_overflow = self.open_overflow.saturating_add(1);
            tracing::debug!(event = "metrics_efficiency_open_window_capped");
        }
        self.open.push_back(Turn {
            creation: self.creation,
            ts: self.last_ts,
            id: message_key,
            model: model_status(input.model),
            usage: input.usage,
        });
        self.creation = self.creation.saturating_add(1);
    }

    fn finalize_turn(&mut self, turn: Turn) {
        if turn.usage.output_tokens == 0 {
            return;
        }
        self.reorder.push(turn);
        if self.reorder.len() > MAX_EFF_REORDER {
            self.emit_earliest();
            self.reorder_overflow = self.reorder_overflow.saturating_add(1);
            tracing::debug!(event = "metrics_efficiency_reorder_window_capped");
        }
    }

    fn emit_earliest(&mut self) {
        let Some((index, _)) = self
            .reorder
            .iter()
            .enumerate()
            .min_by_key(|(_, turn)| (turn.ts, turn.creation))
        else {
            return;
        };
        let turn = self.reorder.swap_remove(index);
        self.fold(turn);
    }

    fn fold(&mut self, turn: Turn) {
        let context = turn.usage.context_tokens();
        let growth = self
            .previous_context
            .map_or(context, |prior| context.saturating_sub(prior));
        self.previous_context = Some(context);
        let contribution = match turn.model {
            ModelStatus::Missing => Contribution::Fallback {
                usage: turn.usage,
                growth,
            },
            ModelStatus::Priced(price) => priced_contribution(turn.usage, growth, &price),
            ModelStatus::Unpriced => Contribution::Unpriced,
        };
        if self.contributions.len() < MAX_EFF_CONTRIBUTIONS {
            self.contributions.push(contribution);
        } else {
            self.contribution_overflow = self.contribution_overflow.saturating_add(1);
            tracing::debug!(event = "metrics_efficiency_contributions_capped");
            fold_overflow(
                contribution,
                &mut self.overflow_totals,
                &mut self.fallback_overflow,
            );
        }
    }

    pub(crate) fn flush(&mut self) {
        while let Some(turn) = self.open.pop_front() {
            self.finalize_turn(turn);
        }
        while !self.reorder.is_empty() {
            self.emit_earliest();
        }
        self.open.shrink_to_fit();
        self.reorder.shrink_to_fit();
    }

    pub(crate) fn finish(mut self, fallback_model: Option<&str>) -> EfficiencyTotals {
        self.flush();
        let fallback = model_status(fallback_model);
        let mut totals = EfficiencyTotals::default();
        for contribution in self.contributions {
            apply_contribution(&mut totals, contribution, &fallback);
        }
        totals.add(self.overflow_totals);
        apply_fallback_overflow(&mut totals, self.fallback_overflow, &fallback);
        totals
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.open
            .capacity()
            .saturating_mul(size_of::<Turn>())
            .saturating_add(self.reorder.capacity().saturating_mul(size_of::<Turn>()))
            .saturating_add(
                self.contributions
                    .capacity()
                    .saturating_mul(size_of::<Contribution>()),
            )
    }
}

fn model_status(model: Option<&str>) -> ModelStatus {
    let Some(model) = model else {
        return ModelStatus::Missing;
    };
    let model = strip_window_tag(model).trim();
    if model.is_empty() {
        return ModelStatus::Unpriced;
    }
    lookup_pricing(model).map_or(ModelStatus::Unpriced, ModelStatus::Priced)
}

fn priced_contribution(usage: Usage, growth: u64, price: &ModelPricing) -> Contribution {
    let fresh = usage
        .input_tokens
        .saturating_add(usage.cache_creation_tokens);
    let new_tokens = fresh.min(growth);
    let rewrite_tokens = fresh.saturating_sub(new_tokens);
    let fresh_rate = if fresh == 0 {
        0.0
    } else {
        (usage.input_tokens as f64 * price.input_cost_per_token
            + usage.cache_creation_tokens as f64 * price.cache_write_cost_per_token)
            / fresh as f64
    };
    Contribution::Priced {
        new_work: usage.output_tokens as f64 * price.output_cost_per_token
            + new_tokens as f64 * fresh_rate,
        carry: usage.cache_read_tokens as f64 * price.cache_read_cost_per_token,
        rewrite: rewrite_tokens as f64 * fresh_rate,
        growth,
        output: usage.output_tokens,
    }
}

fn apply_contribution(
    totals: &mut EfficiencyTotals,
    contribution: Contribution,
    fallback: &ModelStatus,
) {
    match contribution {
        Contribution::Priced {
            new_work,
            carry,
            rewrite,
            growth,
            output,
        } => add_priced_values(totals, new_work, carry, rewrite, growth, output, 1),
        Contribution::Fallback { usage, growth } => match fallback {
            ModelStatus::Priced(price) => {
                if let Contribution::Priced {
                    new_work,
                    carry,
                    rewrite,
                    output,
                    ..
                } = priced_contribution(usage, growth, price)
                {
                    add_priced_values(totals, new_work, carry, rewrite, growth, output, 1);
                }
            }
            ModelStatus::Missing | ModelStatus::Unpriced => {
                totals.unpriced_turns = totals.unpriced_turns.saturating_add(1);
            }
        },
        Contribution::Unpriced => {
            totals.unpriced_turns = totals.unpriced_turns.saturating_add(1);
        }
    }
}

fn add_priced_values(
    totals: &mut EfficiencyTotals,
    new_work: f64,
    carry: f64,
    rewrite: f64,
    growth: u64,
    output: u64,
    turns: u64,
) {
    totals.new_work_usd += new_work;
    totals.carry_usd += carry;
    totals.rewrite_usd += rewrite;
    totals.total_usd += new_work + carry + rewrite;
    totals.growth_tokens = totals.growth_tokens.saturating_add(growth);
    totals.output_tokens = totals.output_tokens.saturating_add(output);
    totals.priced_turns = totals.priced_turns.saturating_add(turns);
}

fn fold_overflow(
    contribution: Contribution,
    totals: &mut EfficiencyTotals,
    fallback: &mut FallbackOverflow,
) {
    match contribution {
        Contribution::Priced {
            new_work,
            carry,
            rewrite,
            growth,
            output,
        } => add_priced_values(totals, new_work, carry, rewrite, growth, output, 1),
        Contribution::Fallback { usage, growth } => add_fallback_overflow(fallback, usage, growth),
        Contribution::Unpriced => {
            totals.unpriced_turns = totals.unpriced_turns.saturating_add(1);
        }
    }
}

fn add_fallback_overflow(target: &mut FallbackOverflow, usage: Usage, growth: u64) {
    let fresh = usage
        .input_tokens
        .saturating_add(usage.cache_creation_tokens);
    let new_tokens = fresh.min(growth);
    let rewrite_tokens = fresh.saturating_sub(new_tokens);
    let input_share = if fresh == 0 {
        0.0
    } else {
        usage.input_tokens as f64 / fresh as f64
    };
    target.growth_tokens = target.growth_tokens.saturating_add(growth);
    target.output_tokens = target.output_tokens.saturating_add(usage.output_tokens);
    target.new_input_tokens += new_tokens as f64 * input_share;
    target.new_cache_tokens += new_tokens as f64 * (1.0 - input_share);
    target.rewrite_input_tokens += rewrite_tokens as f64 * input_share;
    target.rewrite_cache_tokens += rewrite_tokens as f64 * (1.0 - input_share);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    target.turns = target.turns.saturating_add(1);
}

fn apply_fallback_overflow(
    totals: &mut EfficiencyTotals,
    contribution: FallbackOverflow,
    fallback: &ModelStatus,
) {
    let ModelStatus::Priced(price) = fallback else {
        totals.unpriced_turns = totals.unpriced_turns.saturating_add(contribution.turns);
        return;
    };
    let new_work = contribution.output_tokens as f64 * price.output_cost_per_token
        + contribution.new_input_tokens * price.input_cost_per_token
        + contribution.new_cache_tokens * price.cache_write_cost_per_token;
    let carry = contribution.cache_read_tokens as f64 * price.cache_read_cost_per_token;
    let rewrite = contribution.rewrite_input_tokens * price.input_cost_per_token
        + contribution.rewrite_cache_tokens * price.cache_write_cost_per_token;
    add_priced_values(
        totals,
        new_work,
        carry,
        rewrite,
        contribution.growth_tokens,
        contribution.output_tokens,
        contribution.turns,
    );
}

/// Returns efficiency totals for one thread.
///
/// Records with one `message_id` form one turn. Each record without an id forms its own turn.
/// Turns use timestamp order. An untimestamped turn follows the last timestamped turn before it.
/// A turn without output does not contribute. `fallback_model` prices a turn without its own model.
pub fn thread_efficiency(
    events: &[NormalizedEvent],
    fallback_model: Option<&str>,
) -> EfficiencyTotals {
    thread_efficiency_from_inputs(
        events.iter().map(|event| EfficiencyInput {
            ts_ms: event.ts_ms,
            role: event.role,
            message_id: event.message_id.as_deref(),
            model: event.model.as_deref(),
            usage: event.usage,
        }),
        fallback_model,
    )
}

pub(crate) fn thread_efficiency_from_inputs<'a>(
    events: impl IntoIterator<Item = EfficiencyInput<'a>>,
    fallback_model: Option<&str>,
) -> EfficiencyTotals {
    let mut reducer = EfficiencyReducer::default();
    for event in events {
        reducer.observe(event);
    }
    reducer.finish(fallback_model)
}
#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "claude-opus-4-6";

    fn turn(
        ts: i64,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> NormalizedEvent {
        let mut ev = NormalizedEvent::new(Role::Assistant);
        ev.ts_ms = Some(ts);
        ev.model = Some(MODEL.to_string());
        ev.usage = Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_write,
        };
        ev
    }

    fn price() -> crate::pricing::ModelPricing {
        lookup_pricing(MODEL).expect("the test model has a price")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn first_turn_is_all_new_work() {
        let totals = thread_efficiency(&[turn(1, 1_000, 200, 0, 5_000)], None);
        let p = price();
        let expected = 200.0 * p.output_cost_per_token
            + 1_000.0 * p.input_cost_per_token
            + 5_000.0 * p.cache_write_cost_per_token;
        assert!(close(totals.new_work_usd, expected));
        assert!(close(totals.carry_usd, 0.0));
        assert!(close(totals.rewrite_usd, 0.0));
        assert!(close(totals.total_usd, expected));
        assert_eq!(totals.growth_tokens, 6_000);
        assert_eq!(totals.output_tokens, 200);
        assert_eq!(totals.priced_turns, 1);
        assert_eq!(totals.unpriced_turns, 0);
    }

    #[test]
    fn steady_growth_with_cache_reads_is_carry() {
        // The second turn re-reads the 6_000 cached tokens and adds 500 more.
        let events = [turn(1, 1_000, 200, 0, 5_000), turn(2, 100, 50, 6_000, 400)];
        let totals = thread_efficiency(&events, None);
        let p = price();
        let carry = 6_000.0 * p.cache_read_cost_per_token;
        let second_new = 50.0 * p.output_cost_per_token
            + 100.0 * p.input_cost_per_token
            + 400.0 * p.cache_write_cost_per_token;
        assert!(close(totals.carry_usd, carry));
        assert!(close(totals.rewrite_usd, 0.0));
        assert_eq!(totals.growth_tokens, 6_500);
        let first = thread_efficiency(&events[..1], None);
        assert!(close(totals.new_work_usd, first.new_work_usd + second_new));
    }

    #[test]
    fn rebuild_after_a_compaction_drop_is_rewrite_beyond_growth() {
        // Context falls from 10_000 to 4_000. The rebuild sends 4_000 fresh
        // tokens, yet the context did not grow, so every fresh token is a
        // rewrite.
        let events = [turn(1, 0, 10, 0, 10_000), turn(2, 4_000, 10, 0, 0)];
        let totals = thread_efficiency(&events, None);
        let p = price();
        assert!(close(totals.rewrite_usd, 4_000.0 * p.input_cost_per_token));
        assert_eq!(totals.growth_tokens, 10_000);

        // A rebuild that also grows the context splits at the growth.
        let events = [turn(1, 0, 10, 0, 10_000), turn(2, 12_000, 10, 0, 0)];
        let totals = thread_efficiency(&events, None);
        assert!(close(totals.rewrite_usd, 10_000.0 * p.input_cost_per_token));
        assert_eq!(totals.growth_tokens, 12_000);
    }

    #[test]
    fn a_turn_with_no_output_is_dropped() {
        let events = [turn(1, 1_000, 200, 0, 0), turn(2, 9_000, 0, 0, 0)];
        let totals = thread_efficiency(&events, None);
        assert_eq!(totals.priced_turns, 1);
        assert_eq!(totals.unpriced_turns, 0);
        assert_eq!(totals.growth_tokens, 1_000);
    }

    #[test]
    fn an_unpriced_model_is_excluded_from_every_sum() {
        let mut unknown = turn(2, 5_000, 300, 0, 0);
        unknown.model = Some("mystery-model-9".to_string());
        let events = [turn(1, 1_000, 200, 0, 0), unknown];
        let totals = thread_efficiency(&events, None);
        let only_first = thread_efficiency(&events[..1], None);
        assert_eq!(totals.unpriced_turns, 1);
        assert_eq!(totals.priced_turns, 1);
        assert!(close(totals.total_usd, only_first.total_usd));
        assert_eq!(totals.growth_tokens, only_first.growth_tokens);
        assert_eq!(totals.output_tokens, only_first.output_tokens);
    }

    #[test]
    fn the_three_parts_sum_to_the_all_in_cost() {
        let events = [
            turn(1, 1_000, 200, 0, 5_000),
            turn(2, 300, 80, 6_000, 700),
            turn(3, 8_000, 40, 2_000, 0),
        ];
        let totals = thread_efficiency(&events, None);
        let p = price();
        let all_in: f64 = events
            .iter()
            .map(|ev| {
                let u = ev.usage;
                u.input_tokens as f64 * p.input_cost_per_token
                    + u.output_tokens as f64 * p.output_cost_per_token
                    + u.cache_read_tokens as f64 * p.cache_read_cost_per_token
                    + u.cache_creation_tokens as f64 * p.cache_write_cost_per_token
            })
            .sum();
        assert!(close(
            totals.new_work_usd + totals.carry_usd + totals.rewrite_usd,
            all_in
        ));
        assert!(close(totals.total_usd, all_in));
    }

    #[test]
    fn turns_are_ordered_by_timestamp_and_fall_back_to_the_session_model() {
        let mut later = turn(2, 100, 10, 6_000, 0);
        later.model = None;
        let events = [later, turn(1, 1_000, 200, 0, 5_000)];
        let totals = thread_efficiency(&events, Some(MODEL));
        assert_eq!(totals.priced_turns, 2);
        assert_eq!(totals.growth_tokens, 6_100);
        assert!(close(totals.rewrite_usd, 0.0));
    }

    #[test]
    fn add_sums_every_field() {
        let mut a = thread_efficiency(&[turn(1, 1_000, 200, 0, 0)], None);
        let b = thread_efficiency(&[turn(1, 2_000, 300, 0, 0)], None);
        let expected_total = a.total_usd + b.total_usd;
        a.add(b);
        assert!(close(a.total_usd, expected_total));
        assert_eq!(a.growth_tokens, 3_000);
        assert_eq!(a.output_tokens, 500);
        assert_eq!(a.priced_turns, 2);
    }

    #[test]
    fn records_of_one_message_merge_into_one_turn() {
        // The Claude adapter makes the records of one message additive: the
        // second record carries only the output delta. Two turns of 10_000
        // context each must give a growth of 10_000, not 20_000.
        let mut a = turn(1, 0, 100, 10_000, 0);
        a.message_id = Some("msg-a".to_string());
        let mut a_tail = turn(2, 0, 20, 0, 0);
        a_tail.message_id = Some("msg-a".to_string());
        let mut b = turn(3, 0, 50, 10_000, 0);
        b.message_id = Some("msg-b".to_string());
        let totals = thread_efficiency(&[a, a_tail, b], None);
        assert_eq!(totals.priced_turns, 2);
        assert_eq!(totals.growth_tokens, 10_000);
        assert_eq!(totals.output_tokens, 170);
    }

    #[test]
    fn a_later_fragment_backfills_the_message_model() {
        let mut first = turn(1, 100, 10, 0, 0);
        first.message_id = Some("message".to_string());
        first.model = None;
        let mut tail = turn(2, 0, 5, 0, 0);
        tail.message_id = Some("message".to_string());
        let totals = thread_efficiency(&[first, tail], None);
        assert_eq!(totals.priced_turns, 1);
        assert_eq!(totals.output_tokens, 15);
    }

    #[test]
    fn non_assistant_records_advance_the_carried_timestamp() {
        let first = turn(10, 100, 10, 0, 0);
        let mut user = NormalizedEvent::new(Role::User);
        user.ts_ms = Some(5);
        let mut untimestamped = turn(20, 100, 10, 0, 0);
        untimestamped.ts_ms = None;
        let totals = thread_efficiency(&[first, user, untimestamped], None);
        assert_eq!(totals.priced_turns, 2);
        assert_eq!(totals.growth_tokens, 100);
    }

    #[test]
    fn an_empty_model_blocks_the_fallback_model() {
        let mut current = turn(1, 100, 10, 0, 0);
        current.model = Some("   ".to_string());
        let totals = thread_efficiency(&[current], Some(MODEL));
        assert_eq!(totals.priced_turns, 0);
        assert_eq!(totals.unpriced_turns, 1);
    }

    #[test]
    fn fallback_turns_keep_reference_float_order() {
        let mut events = Vec::new();
        for index in 0..100 {
            let mut current = turn(
                index,
                101 + index as u64,
                17 + index as u64,
                503 + index as u64,
                211 + index as u64,
            );
            current.model = None;
            events.push(current);
        }
        let actual = thread_efficiency(&events, Some(MODEL));
        let mut expected = EfficiencyTotals::default();
        let mut previous_context = None;
        let price = price();
        for current in &events {
            let context = current.usage.context_tokens();
            let growth =
                previous_context.map_or(context, |prior: u64| context.saturating_sub(prior));
            previous_context = Some(context);
            if let Contribution::Priced {
                new_work,
                carry,
                rewrite,
                output,
                ..
            } = priced_contribution(current.usage, growth, &price)
            {
                add_priced_values(&mut expected, new_work, carry, rewrite, growth, output, 1);
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn long_message_ids_do_not_collide_on_a_shared_prefix() {
        let prefix = "x".repeat(80);
        let mut first = turn(1, 10, 2, 0, 0);
        first.message_id = Some(format!("{prefix}-a"));
        let mut second = turn(2, 20, 3, 0, 0);
        second.message_id = Some(format!("{prefix}-b"));
        let totals = thread_efficiency(&[first, second], None);
        assert_eq!(totals.priced_turns, 2);
    }

    #[test]
    fn efficiency_overflow_counters_report_each_degradation() {
        let mut reducer = EfficiencyReducer::default();
        for index in 0..(MAX_EFF_CONTRIBUTIONS + MAX_OPEN_MESSAGES + MAX_EFF_REORDER + 10) {
            let message_id = format!("message-{index}");
            reducer.observe(EfficiencyInput {
                ts_ms: Some(index as i64),
                role: Role::Assistant,
                message_id: Some(&message_id),
                model: Some(MODEL),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                },
            });
        }
        reducer.flush();
        assert!(reducer.open_overflow > 0);
        assert!(reducer.reorder_overflow > 0);
        assert!(reducer.contribution_overflow > 0);
    }

    #[test]
    fn a_message_recurring_past_the_open_window_becomes_a_new_turn() {
        let mut events = Vec::new();
        for index in 0..=MAX_OPEN_MESSAGES {
            let mut current = turn(index as i64, 1, 1, 0, 0);
            current.message_id = Some(format!("message-{index}"));
            events.push(current);
        }
        let mut recurrence = turn(100, 1, 1, 0, 0);
        recurrence.message_id = Some("message-0".to_string());
        events.push(recurrence);
        let totals = thread_efficiency(&events, None);
        assert_eq!(totals.priced_turns, (MAX_OPEN_MESSAGES + 2) as u64);
    }
}
