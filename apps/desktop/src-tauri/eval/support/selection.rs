use std::collections::BTreeSet;

pub(crate) fn suite() -> String {
    optional_env("ANTIBURN_EVAL_SUITE").unwrap_or_else(|| "development".into())
}

fn optional_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} must be UTF-8"),
    }
}

pub(crate) fn select<T>(cases: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
    let selected = optional_env("ANTIBURN_EVAL_CASES").map(|value| {
        value
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    });
    let limit = optional_env("ANTIBURN_EVAL_LIMIT").map(|value| {
        let value = value
            .parse::<usize>()
            .expect("ANTIBURN_EVAL_LIMIT is an integer");
        assert!(value > 0, "ANTIBURN_EVAL_LIMIT must be positive");
        value
    });
    select_ids(cases, id, selected.as_ref(), limit)
}

fn select_ids<T>(
    cases: Vec<T>,
    id: impl Fn(&T) -> &str,
    selected: Option<&BTreeSet<String>>,
    limit: Option<usize>,
) -> Vec<T> {
    if let Some(selected) = selected {
        let known: BTreeSet<_> = cases.iter().map(|case| id(case).to_owned()).collect();
        assert!(
            selected.is_subset(&known),
            "ANTIBURN_EVAL_CASES contains an unknown case ID"
        );
    }
    let mut cases: Vec<_> = cases
        .into_iter()
        .filter(|case| selected.is_none_or(|selected| selected.contains(id(case))))
        .collect();
    if let Some(limit) = limit {
        cases.truncate(limit);
    }
    assert!(!cases.is_empty(), "Select at least one evaluation case");
    cases
}

#[test]
fn case_selection_preserves_order_and_applies_the_limit_after_ids() {
    let ids = BTreeSet::from(["b".into(), "c".into()]);
    assert_eq!(
        select_ids(vec!["a", "b", "c"], |case| case, Some(&ids), Some(1)),
        vec!["b"]
    );
}

#[test]
#[should_panic(expected = "unknown case ID")]
fn unknown_case_ids_do_not_fall_back_to_the_full_suite() {
    let ids = BTreeSet::from(["missing".into()]);
    select_ids(vec!["a"], |case| case, Some(&ids), None);
}
