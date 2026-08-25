// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Spend efficiency for one thread of turns.
//!
//! Every dollar a turn costs goes to one of three places. *New work* is the
//! output plus the fresh input that grew the context. *Carry* is the cached
//! prefix the model re-reads. *Rewrite* is fresh input that did not grow the
//! context: the prefix the agent sent again after a compaction, a cache miss,
//! or a model switch. The split uses only the usage counters and the pricing
//! table, so it needs no transcript text.
//!
//! A thread is one event stream. The parent transcript is one thread, and
//! each sub-agent transcript is its own thread. The context of one thread
//! says nothing about another, so a caller sums per-thread totals with
//! [`EfficiencyTotals::add`] instead of running one pass over a merged stream.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analysis::model::{NormalizedEvent, Role, Usage};
use crate::analysis::pricing::{lookup_pricing, strip_window_tag};

/// Additive spend totals for one or more threads.
///
/// Every field sums over priced turns only. A turn whose model has no price
/// counts in `unpriced_turns` and in nothing else, so a ratio of two fields
/// always describes the same set of turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EfficiencyTotals {
    /// The all-in cost of the priced turns.
    pub total_usd: f64,
    /// Output plus the fresh input that grew the context.
    pub new_work_usd: f64,
    /// Cache reads.
    pub carry_usd: f64,
    /// Fresh input beyond the context growth.
    pub rewrite_usd: f64,
    /// Context growth summed over the priced turns.
    pub growth_tokens: u64,
    /// Output tokens summed over the priced turns.
    pub output_tokens: u64,
    pub priced_turns: u64,
    /// Turns with output whose model has no price.
    pub unpriced_turns: u64,
}

impl EfficiencyTotals {
    /// Add another thread's totals into this one.
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

/// One assistant turn after the records of one message are merged.
struct Turn<'a> {
    ts: i64,
    model: Option<&'a str>,
    usage: Usage,
}

/// The efficiency totals for one thread of events.
///
/// A Claude transcript writes one record per content block of a message,
/// and the vendor adapter makes their usage additive. The function sums
/// the records that share a `message_id` back into one turn. A record with
/// no `message_id` is a turn on its own. The function orders the turns by
/// `ts_ms`. A turn with no timestamp keeps its place after the last
/// timestamped turn before it. A turn with no output is not a turn for
/// this purpose. `fallback_model` prices a turn that records no model of
/// its own.
pub fn thread_efficiency(
    events: &[NormalizedEvent],
    fallback_model: Option<&str>,
) -> EfficiencyTotals {
    let mut turns: Vec<Turn<'_>> = Vec::new();
    let mut index_by_id: HashMap<&str, usize> = HashMap::new();
    let mut last_ts = i64::MIN;
    for ev in events {
        if let Some(ts) = ev.ts_ms {
            last_ts = ts;
        }
        if ev.role != Role::Assistant {
            continue;
        }
        if let Some(&i) = ev.message_id.as_deref().and_then(|id| index_by_id.get(id)) {
            let turn = &mut turns[i];
            turn.usage = turn.usage.saturating_add(ev.usage);
            if turn.model.is_none() {
                turn.model = ev.model.as_deref();
            }
            continue;
        }
        if let Some(id) = ev.message_id.as_deref() {
            index_by_id.insert(id, turns.len());
        }
        turns.push(Turn {
            ts: last_ts,
            model: ev.model.as_deref(),
            usage: ev.usage,
        });
    }
    turns.retain(|t| t.usage.output_tokens > 0);
    turns.sort_by_key(|t| t.ts);

    let mut totals = EfficiencyTotals::default();
    let mut prev_ctx: Option<u64> = None;
    for turn in turns {
        let u = turn.usage;
        let ctx = u.context_tokens();
        let growth = match prev_ctx {
            None => ctx,
            Some(prev) => ctx.saturating_sub(prev),
        };
        prev_ctx = Some(ctx);

        let model = turn
            .model
            .or(fallback_model)
            .map(|m| strip_window_tag(m).trim())
            .filter(|m| !m.is_empty());
        let Some(price) = model.and_then(lookup_pricing) else {
            totals.unpriced_turns += 1;
            continue;
        };

        let fresh = u.input_tokens.saturating_add(u.cache_creation_tokens);
        let new_tok = fresh.min(growth);
        let rewrite_tok = fresh - new_tok;
        // The fresh rate blends the input and cache-write rates in the same
        // proportion this turn sent them.
        let fresh_rate = if fresh == 0 {
            0.0
        } else {
            (u.input_tokens as f64 * price.input_cost_per_token
                + u.cache_creation_tokens as f64 * price.cache_write_cost_per_token)
                / fresh as f64
        };
        let new_work_usd =
            u.output_tokens as f64 * price.output_cost_per_token + new_tok as f64 * fresh_rate;
        let carry_usd = u.cache_read_tokens as f64 * price.cache_read_cost_per_token;
        let rewrite_usd = rewrite_tok as f64 * fresh_rate;

        totals.new_work_usd += new_work_usd;
        totals.carry_usd += carry_usd;
        totals.rewrite_usd += rewrite_usd;
        totals.total_usd += new_work_usd + carry_usd + rewrite_usd;
        totals.growth_tokens = totals.growth_tokens.saturating_add(growth);
        totals.output_tokens = totals.output_tokens.saturating_add(u.output_tokens);
        totals.priced_turns += 1;
    }
    totals
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
}
