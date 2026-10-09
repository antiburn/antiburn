use std::collections::BTreeSet;

use serde_json::{Value, json};

pub(crate) fn ratio(numerator: usize, denominator: usize) -> Value {
    if denominator == 0 {
        Value::Null
    } else {
        json!(numerator as f64 / denominator as f64)
    }
}

pub(crate) fn inventory_errors(schedule: &[Value], rows: &[Value]) -> usize {
    let ids = |values: &[Value]| {
        values
            .iter()
            .filter_map(|row| row["id"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>()
    };
    let scheduled = ids(schedule);
    let observed = ids(rows);
    schedule.len() - scheduled.len() + rows.len() - observed.len()
        + scheduled.symmetric_difference(&observed).count()
}

#[test]
fn exact_inventory_rejects_duplicates_unknown_ids_and_missing_rows() {
    let schedule = vec![json!({"id":"a"}), json!({"id":"b"})];
    let rows = vec![json!({"id":"a"}), json!({"id":"a"}), json!({"id":"c"})];
    assert_eq!(inventory_errors(&schedule, &rows), 3);
    assert_eq!(ratio(0, 0), Value::Null);
}
