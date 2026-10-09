//! Select normalized human and command-result facts for an instruction episode.

use super::ContentAction;

pub(super) fn human_text(action: &ContentAction) -> bool {
    action
        .metadata
        .human_text
        .as_ref()
        .is_some_and(|fact| fact.matches_action(action))
}

pub(super) fn command_result(action: &ContentAction, actions: &[ContentAction]) -> bool {
    action.metadata.command_result.as_ref().is_some_and(|fact| {
        fact.matches_action(action) && actions.iter().any(|request| fact.matches_request(request))
    })
}

pub(super) fn supported(action: &ContentAction, actions: &[ContentAction]) -> bool {
    (action.authority == "assistant"
        && (super::action_context::is_assistant_text(&action.kind) || action.kind == "tool_input"))
        || human_text(action)
        || command_result(action, actions)
}

pub(super) fn supporting_text_ranges(
    text: &str,
    max_bytes: usize,
    target: &str,
) -> Vec<(usize, usize)> {
    if max_bytes == 0 || text.is_empty() {
        return Vec::new();
    }
    if text.len() <= max_bytes {
        return vec![(0, text.len())];
    }
    let unit = (max_bytes.saturating_sub(2) / 3).max(1);
    let ranges = crate::analysis::jev::text_ranges::text_ranges(text, unit, 0);
    let terms = super::planning::meaningful_terms(target);
    let relevant = ranges
        .iter()
        .enumerate()
        .max_by_key(|(_, (start, end))| {
            let chunk_terms = super::planning::meaningful_terms(&text[*start..*end]);
            terms.intersection(&chunk_terms).count()
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    let mut selected = vec![relevant, 0, ranges.len() - 1];
    selected.sort_unstable();
    selected.dedup();
    let mut remaining = max_bytes;
    let mut selected_count = 0;
    selected
        .into_iter()
        .filter_map(|index| {
            let range = ranges[index];
            let bytes = range.1 - range.0;
            let cost = bytes + usize::from(selected_count > 0);
            if cost > remaining {
                None
            } else {
                remaining -= cost;
                selected_count += 1;
                Some(range)
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests;
