use crate::support::run::RunUsage;
use antiburn_local::analysis::jev::{JevCheck, JevCheckPlan};
use std::sync::{Arc, Mutex};

pub(crate) fn result<C: JevCheck>(
    check: &C,
    plan: &JevCheckPlan<C::Prepared>,
    usage: &Arc<Mutex<RunUsage>>,
    case_id: &str,
) -> Option<Result<C::Result, String>> {
    std::env::var_os("ANTIBURN_EVAL_BASELINE_QUESTIONS")?;
    let totals = usage.lock().expect("usage mutex is not poisoned");
    if let Some(error) = totals.baseline_errors.get(case_id) {
        return Some(Err(error.clone()));
    }
    let results = totals
        .baseline_results
        .get(case_id)
        .map_or(&[][..], Vec::as_slice);
    let selected = results
        .iter()
        .filter(|result| {
            plan.work_items
                .iter()
                .any(|item| item.id == result.work_item_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let complete = plan.skipped_item_ids.is_empty() && selected.len() == plan.work_items.len();
    Some(
        check
            .reduce(plan, &selected, complete)
            .map_err(|error| error.to_string()),
    )
}
