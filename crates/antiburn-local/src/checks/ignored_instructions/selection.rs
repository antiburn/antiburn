//! Deterministic sampling with bounded score retention.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::*;

const SCORE_BATCH: usize = 16;

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
        let (scored, retained, identities) = counts.get();
        counts.set((scored, retained, identities + 1));
    });
}

struct ScoreFacts {
    terms: Vec<BTreeSet<String>>,
    document_frequency: BTreeMap<String, usize>,
    paths: Vec<Vec<String>>,
    tool_names: Vec<Option<String>>,
    all_tool_names: BTreeSet<String>,
    all_paths: BTreeSet<String>,
    has_tool_inputs: bool,
}

struct RuleScore<'a> {
    text: &'a str,
    lower_text: String,
    terms: &'a BTreeSet<String>,
    prohibits: bool,
}

impl<'a> RuleScore<'a> {
    fn new(rule: RuleRange<'a>, terms: &'a BTreeSet<String>) -> Self {
        let (_, rule, start, end) = rule;
        let text = &rule.text[start..end];
        Self {
            text,
            lower_text: text.to_ascii_lowercase(),
            terms,
            prohibits: ["never", "Never", "Do not", "must not"]
                .iter()
                .any(|word| text.contains(word)),
        }
    }
}

impl ScoreFacts {
    fn new(actions: &[ActionRange<'_>], index: &ComparisonIndex<'_>) -> Self {
        let terms = actions
            .iter()
            .map(|(action, start, end)| {
                let mut terms = meaningful_terms(&action.text[*start..*end]);
                terms.extend(index.action_terms[&action.reference.id].iter().cloned());
                if let Some(fields) = &action.normalized_fields {
                    for value in fields.values.values() {
                        terms.extend(meaningful_terms(value));
                    }
                }
                terms
            })
            .collect::<Vec<_>>();
        let mut document_frequency = BTreeMap::new();
        for terms in &terms {
            for term in terms {
                *document_frequency.entry(term.clone()).or_default() += 1;
            }
        }
        let paths = actions
            .iter()
            .map(|(action, _, _)| {
                ExactActionFacts::from_selected(
                    action.tool_name.as_deref(),
                    action.normalized_fields.as_ref(),
                )
                .paths
            })
            .collect::<Vec<_>>();
        let tool_names = actions
            .iter()
            .map(|(action, _, _)| {
                action
                    .tool_name
                    .as_ref()
                    .map(|name| name.to_ascii_lowercase())
            })
            .collect::<Vec<_>>();
        let all_tool_names = tool_names.iter().flatten().cloned().collect();
        let all_paths = paths.iter().flatten().cloned().collect();
        Self {
            terms,
            document_frequency,
            paths,
            tool_names,
            all_tool_names,
            all_paths,
            has_tool_inputs: actions
                .iter()
                .any(|(action, _, _)| action.kind == "tool_input"),
        }
    }

    fn score(
        &self,
        rule: &RuleScore<'_>,
        actions: &[ActionRange<'_>],
        action_index: usize,
    ) -> usize {
        #[cfg(test)]
        SELECTION_COUNTS.with(|counts| {
            let (scored, retained, identities) = counts.get();
            counts.set((scored + 1, retained, identities));
        });
        let action = actions[action_index].0;
        let overlap = rule
            .terms
            .iter()
            .filter(|term| self.terms[action_index].contains(*term))
            .map(|term| 1000 / (1 + self.document_frequency[term]))
            .sum::<usize>();
        let tool_match = self.tool_names[action_index]
            .as_ref()
            .is_some_and(|name| rule.lower_text.contains(name));
        let path_match = self.paths[action_index]
            .iter()
            .any(|path| rule.text.contains(path));
        let risk = usize::from(rule.prohibits) * usize::from(action.kind == "tool_input");
        overlap.saturating_mul(8)
            + usize::from(tool_match) * 800
            + usize::from(path_match) * 1200
            + risk * 12
            + action_index * 16 / actions.len()
    }

    fn priority_possible(&self, rule: &RuleScore<'_>) -> bool {
        rule.terms.iter().any(|term| {
            self.document_frequency
                .get(term)
                .is_some_and(|frequency| 1000 / (1 + frequency) > 0)
        }) || self
            .all_tool_names
            .iter()
            .any(|name| rule.lower_text.contains(name))
            || self.all_paths.iter().any(|path| rule.text.contains(path))
            || rule.prohibits && self.has_tool_inputs
    }
}

struct CandidateFilter<'a> {
    new_only: bool,
    probe: Option<bool>,
    cutoff: Option<(usize, usize)>,
    ledger: &'a SamplingLedger,
    excluded: &'a BTreeSet<(usize, usize)>,
    used: &'a BTreeSet<(usize, usize)>,
    action_counts: Option<&'a [usize]>,
}

fn score_batch(
    rule_index: usize,
    rules: &[RuleRange<'_>],
    actions: &[ActionRange<'_>],
    index: &ComparisonIndex<'_>,
    facts: &ScoreFacts,
    filter: &CandidateFilter<'_>,
    limit: usize,
) -> Vec<(usize, usize)> {
    let mut best = BinaryHeap::with_capacity(limit + 1);
    let rule = RuleScore::new(rules[rule_index], &index.rule_terms[rule_index]);
    // Skip only the high-score bucket when its upper bound is below 16.
    if filter.probe == Some(false) && !facts.priority_possible(&rule) {
        return Vec::new();
    }
    for (action_index, (action, _, _)) in actions.iter().enumerate() {
        if filter.excluded.contains(&(rule_index, action_index))
            || filter.used.contains(&(rule_index, action_index))
            || filter.new_only
                && filter
                    .ledger
                    .known_action_ids
                    .contains(&action.reference.id)
            || filter
                .action_counts
                .is_some_and(|counts| counts[action_index] >= 3)
        {
            continue;
        }
        let score = facts.score(&rule, actions, action_index);
        if filter.probe.is_some_and(|probe| (score < 16) != probe)
            || filter
                .cutoff
                .is_some_and(|cutoff| (score, action_index) >= cutoff)
        {
            continue;
        }
        best.push(Reverse((score, action_index)));
        if best.len() > limit {
            best.pop();
        }
    }
    let mut best = best
        .into_iter()
        .map(|Reverse(value)| value)
        .collect::<Vec<_>>();
    // Pop the best score first. The action index breaks ties in source order.
    best.sort_unstable();
    best
}

pub(super) fn select_coordinates(
    rules: &[RuleRange<'_>],
    actions: &[ActionRange<'_>],
    ledger: &SamplingLedger,
    reviewed: &BTreeSet<(usize, usize)>,
    index: &ComparisonIndex<'_>,
) -> Vec<(usize, usize)> {
    let facts = ScoreFacts::new(actions, index);
    let limit = MAX_SAMPLED_COMPARISONS_PER_PASS.min(rules.len().saturating_mul(actions.len()));
    let mut chosen = Vec::with_capacity(limit);
    let mut used = BTreeSet::new();
    let mut excluded = reviewed.clone();
    let mut action_counts = vec![0usize; actions.len()];
    let legacy_ids = ledger.comparison_ids.len() > ledger.comparisons.len();
    for new_only in [true, false] {
        if new_only
            && (ledger.known_action_ids.is_empty()
                || actions
                    .iter()
                    .all(|(action, _, _)| ledger.known_action_ids.contains(&action.reference.id)))
        {
            continue;
        }
        for probe in [false, true] {
            for rule_index in 0..rules.len() {
                if chosen.len() == limit {
                    break;
                }
                loop {
                    let best = score_batch(
                        rule_index,
                        rules,
                        actions,
                        index,
                        &facts,
                        &CandidateFilter {
                            new_only,
                            probe: Some(probe),
                            cutoff: None,
                            ledger,
                            excluded: &excluded,
                            used: &used,
                            action_counts: Some(&action_counts),
                        },
                        1,
                    )
                    .pop();
                    let Some((_, action_index)) = best else {
                        break;
                    };
                    if legacy_ids
                        && is_legacy_reviewed(rules, actions, ledger, rule_index, action_index)
                    {
                        excluded.insert((rule_index, action_index));
                        continue;
                    }
                    used.insert((rule_index, action_index));
                    action_counts[action_index] += 1;
                    chosen.push((rule_index, action_index));
                    break;
                }
            }
        }
        let mut batches = BTreeMap::<usize, Vec<(usize, usize)>>::new();
        let mut cutoffs = BTreeMap::new();
        while chosen.len() < limit {
            let mut advanced = false;
            for rule_index in 0..rules.len() {
                loop {
                    let batch = batches.entry(rule_index).or_default();
                    if batch.is_empty() {
                        *batch = score_batch(
                            rule_index,
                            rules,
                            actions,
                            index,
                            &facts,
                            &CandidateFilter {
                                new_only,
                                probe: None,
                                cutoff: cutoffs.get(&rule_index).copied(),
                                ledger,
                                excluded: &excluded,
                                used: &used,
                                action_counts: None,
                            },
                            (limit / rules.len() + SCORE_BATCH).min(limit - chosen.len()),
                        );
                        #[cfg(test)]
                        SELECTION_COUNTS.with(|counts| {
                            let (scored, retained, identities) = counts.get();
                            counts.set((
                                scored,
                                retained.max(batches.values().map(Vec::len).sum()),
                                identities,
                            ));
                        });
                    }
                    let Some((score, action_index)) =
                        batches.get_mut(&rule_index).and_then(Vec::pop)
                    else {
                        break;
                    };
                    cutoffs.insert(rule_index, (score, action_index));
                    if legacy_ids
                        && is_legacy_reviewed(rules, actions, ledger, rule_index, action_index)
                    {
                        excluded.insert((rule_index, action_index));
                        continue;
                    }
                    used.insert((rule_index, action_index));
                    action_counts[action_index] += 1;
                    chosen.push((rule_index, action_index));
                    advanced = true;
                    break;
                }
                if chosen.len() == limit {
                    break;
                }
            }
            if !advanced {
                break;
            }
        }
    }
    chosen
}

fn is_legacy_reviewed(
    rules: &[RuleRange<'_>],
    actions: &[ActionRange<'_>],
    ledger: &SamplingLedger,
    rule_index: usize,
    action_index: usize,
) -> bool {
    let (instruction, rule, start, end) = rules[rule_index];
    let (action, action_start, action_end) = actions[action_index];
    ledger.comparison_ids.contains(&comparison_identity(
        instruction,
        rule,
        action,
        (start, end),
        (action_start, action_end),
    ))
}
