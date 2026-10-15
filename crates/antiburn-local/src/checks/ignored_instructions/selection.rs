//! Select the newest direct comparisons before applying answer reuse.

use super::*;

#[cfg(test)]
std::thread_local! {
    static SELECTION_COUNTS: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
}

#[cfg(test)]
pub(super) fn take_selection_counts() -> (usize, usize, usize) {
    SELECTION_COUNTS.with(|counts| counts.replace((0, 0, 0)))
}

#[cfg(test)]
pub(super) fn record_identity() {
    SELECTION_COUNTS.with(|counts| {
        let (scanned, retained, identities) = counts.get();
        counts.set((scanned, retained, identities + 1));
    });
}

pub(super) fn select_coordinates(
    rules: &[RuleRange<'_>],
    actions: &[ActionRange<'_>],
    ledger: &SamplingLedger,
    reviewed: &BTreeSet<(usize, usize)>,
) -> Vec<(usize, usize)> {
    let mut order = (0..actions.len()).collect::<Vec<_>>();
    order.sort_by(|&left, &right| {
        let left_action = actions[left].0;
        let right_action = actions[right].0;
        right_action
            .timestamp_ms
            .cmp(&left_action.timestamp_ms)
            .then_with(|| {
                right_action
                    .reference
                    .turn_index
                    .cmp(&left_action.reference.turn_index)
            })
            .then_with(|| {
                right_action
                    .reference
                    .part_index
                    .cmp(&left_action.reference.part_index)
            })
            .then_with(|| left.cmp(&right))
    });
    let mut admitted = Vec::with_capacity(MAX_ASSESSMENT_CANDIDATES);
    'actions: for action_index in order {
        for (rule_index, (instruction, rule, start, end)) in rules.iter().enumerate() {
            #[cfg(test)]
            SELECTION_COUNTS.with(|counts| {
                let (scanned, retained, identities) = counts.get();
                counts.set((scanned + 1, retained, identities));
            });
            if instruction_scope_matches(instruction, actions[action_index].0)
                && direct_anchor_match(rule, *start, *end, actions[action_index].0)
            {
                admitted.push((rule_index, action_index));
                if admitted.len() == MAX_ASSESSMENT_CANDIDATES {
                    break 'actions;
                }
            }
        }
    }
    #[cfg(test)]
    SELECTION_COUNTS.with(|counts| {
        let (scanned, _, identities) = counts.get();
        counts.set((scanned, admitted.len(), identities));
    });
    admitted.retain(|&(rule_index, action_index)| {
        if reviewed.contains(&(rule_index, action_index)) {
            return false;
        }
        let (instruction, rule, start, end) = rules[rule_index];
        let (action, action_start, action_end) = actions[action_index];
        !ledger.comparison_ids.contains(&comparison_identity(
            instruction,
            rule,
            action,
            (start, end),
            (action_start, action_end),
        ))
    });
    admitted
}
