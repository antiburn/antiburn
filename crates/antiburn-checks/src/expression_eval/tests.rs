use serde_json::{Value, json};

use super::*;
use crate::expression::{ExpressionFunction, parse_expression};

fn eval(source: &str, context: &Value) -> EvaluationValue {
    evaluate_expression(&parse_expression(source).unwrap(), context).unwrap()
}

fn traced(
    source: &str,
    context: &Value,
    bindings: &EvaluationBindings,
    budget: &mut EvaluationBudget,
) -> EvaluatedValue {
    evaluate_expression_with_budget(
        &parse_expression(source).unwrap(),
        context,
        bindings,
        budget,
        1,
    )
    .unwrap()
}

fn assert_value(source: &str, expected: Value) {
    assert_eq!(
        eval(source, &json!({})),
        EvaluationValue::Available(expected),
        "{source}"
    );
}

fn assert_invalid(source: &str) {
    let error = evaluate_expression(&parse_expression(source).unwrap(), &json!({})).unwrap_err();
    assert_eq!(
        error.code,
        DiagnosticCode::RuntimeInvalidContext,
        "{source}"
    );
}

fn path(properties: &[&str]) -> Vec<PathSegment> {
    properties
        .iter()
        .map(|property| PathSegment::Property((*property).to_owned()))
        .collect()
}

#[test]
fn dynamic_access_checks_keys_indexes_and_own_json_values() {
    let context = json!({"data": {"Key": 7, "key": 8, "constructor": "own", "__proto__": {"safe": true}}, "key": "Key", "index": 1.0, "items": [4, 5]});
    for (source, expected) in [
        ("data[key]", json!(7)),
        ("items[index]", json!(5)),
        ("items[1e0]", json!(5)),
        ("items[-0.0]", json!(4)),
        ("data['key']", json!(8)),
        ("data['constructor']", json!("own")),
        ("data['__proto__'].safe", json!(true)),
    ] {
        assert_eq!(
            eval(source, &context),
            EvaluationValue::Available(expected),
            "{source}"
        );
    }
    for source in [
        "data['KEY']",
        "items[-1]",
        "items[0.5]",
        "items[99]",
        "items['0']",
        "items[true]",
        "items[null]",
        "items[1e100]",
        "data[0]",
        "data.toString",
        "data[key].x",
        "data[missing] || 1",
        "'text'.length",
    ] {
        assert_eq!(
            eval(source, &context),
            EvaluationValue::Unavailable,
            "{source}"
        );
    }
    assert_eq!(
        eval("items.length", &context),
        EvaluationValue::Available(json!(2))
    );
    assert_eq!(eval("data.length", &context), EvaluationValue::Unavailable);
    assert_value("fromJSON('{\"length\":9}').length", json!(9));
}

#[test]
fn calls_use_case_insensitive_names_and_scalar_conversion() {
    for (source, expected) in [
        ("sTrIcTeQuAlS('a', 'A')", json!(false)),
        ("strictEquals('a', 'a')", json!(true)),
        ("strictEquals(false, 0)", json!(false)),
        ("strictEquals(null, null)", json!(true)),
        ("strictEquals(1, 1.0)", json!(true)),
        ("CONTAINS('Alpha Beta', 'bETA')", json!(true)),
        ("contains(120, 2)", json!(true)),
        ("contains(null, '')", json!(true)),
        ("startsWith('Alpha', 'AL')", json!(true)),
        ("endsWITH('Alpha', 'HA')", json!(true)),
        ("startsWith(true, 'TR')", json!(true)),
        ("endsWith(null, '')", json!(true)),
        ("contains(fromJSON('[1,\"A\",null]'), 'a')", json!(true)),
        ("contains(fromJSON('[1,2]'), '2')", json!(true)),
        ("contains(fromJSON('[1,2]'), 3)", json!(false)),
        ("join(fromJSON('[1,true,null,\"x\"]'))", json!("1,true,,x")),
        ("join(fromJSON('[1,2]'), null)", json!("12")),
        ("join(fromJSON('[]'), ':')", json!("")),
        ("join('unchanged', '|')", json!("unchanged")),
        (
            "format('{{{0}}}: {1} {0}', 'x', true)",
            json!("{x}: true x"),
        ),
        ("format('{0}', null)", json!("")),
        ("format(12, 'unused')", json!("12")),
    ] {
        assert_value(source, expected);
    }
}

#[test]
fn invalid_function_values_are_errors_not_empty_fallbacks() {
    for source in [
        "strictEquals(fromJSON('[]'), fromJSON('[]'))",
        "strictEquals(null, fromJSON('{}'))",
        "contains(fromJSON('{}'), 'x')",
        "contains('x', fromJSON('[]'))",
        "startsWith(fromJSON('{}'), 'x')",
        "endsWith('x', fromJSON('{}'))",
        "join(1)",
        "join(fromJSON('[{}]'))",
        "join('text', fromJSON('{}'))",
        "format('{1}', 'x')",
        "format('{', 'x')",
        "format('}', 'x')",
        "format('{}', 'x')",
        "format('{-1}', 'x')",
        "format('{0:2}', 'x')",
        "format('{999999999999999999999999999}', 'x')",
        "format('{0}', fromJSON('[]'))",
        "fromJSON(null)",
        "fromJSON('bad')",
        "fromJSON('true false')",
        "fromJSON('NaN')",
        "fromJSON('1e400')",
        "fromJSON('[1,]')",
    ] {
        assert_invalid(source);
    }
    let expression = Expression::Call {
        function: ExpressionFunction::Join,
        arguments: Vec::new(),
    };
    assert_eq!(
        evaluate_expression(&expression, &json!({}))
            .unwrap_err()
            .code,
        DiagnosticCode::RuntimeInvalidContext
    );
}

#[test]
fn json_serialization_sorts_keys_and_escapes_text() {
    let context =
        json!({"data": {"z": [null, true, 3], "a": {"b": "\"\\\n\r\t\u{08}\u{0c}\u{01}é"}}});
    let EvaluationValue::Available(Value::String(text)) = eval("toJSON(data)", &context) else {
        panic!("JSON text");
    };
    assert_eq!(
        text,
        "{\"a\":{\"b\":\"\\\"\\\\\\n\\r\\t\\b\\f\\u0001é\"},\"z\":[null,true,3]}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        context["data"]
    );
    assert_eq!(
        eval("fromJSON(toJSON(data))", &context),
        EvaluationValue::Available(context["data"].clone())
    );
    assert_value(
        "fromJSON('9007199254740993') == 9007199254740993",
        json!(true),
    );
}

#[test]
fn projections_preserve_order_nested_arrays_and_missing_fields() {
    let context = json!({"groups": [{"items": [{"name": "a"}, {"name": "b"}]}, {"items": [{"name": "c"}]}], "object": {"z": {"name": 2}, "a": {"name": 1}}, "empty": [], "bad": [{"name": 1}, {}]});
    assert_eq!(
        eval("groups.*.items.*.name", &context),
        EvaluationValue::Available(json!([["a", "b"], ["c"]]))
    );
    assert_eq!(
        eval("object.*.name", &context),
        EvaluationValue::Available(json!([1, 2]))
    );
    assert_eq!(
        eval("empty.*.name", &context),
        EvaluationValue::Available(json!([]))
    );
    assert_eq!(eval("bad.*.name", &context), EvaluationValue::Unavailable);
    assert_eq!(eval("(1).*.name", &context), EvaluationValue::Unavailable);
    assert_eq!(
        eval("(groups.*.items)[1].length", &context),
        EvaluationValue::Available(json!(1))
    );
    assert_eq!(
        eval("((groups.*.items))[0][1].name", &context),
        EvaluationValue::Available(json!("b"))
    );
    assert_eq!(
        eval("(groups.*.items).*.length", &context),
        EvaluationValue::Available(json!([2, 1]))
    );
    assert_value("fromJSON('{\"b\":2,\"a\":1}').*", json!([1, 2]));
}

#[test]
fn evaluated_missing_data_stays_unavailable_and_skipped_branches_are_unused() {
    for source in [
        "contains(missing, 'x') || true",
        "format('{0}', missing) == null",
        "toJSON(missing)",
        "fromJSON(missing)",
        "false || missing",
        "missing && false",
        "missing == null",
        "!missing",
    ] {
        assert_eq!(
            eval(source, &json!({})),
            EvaluationValue::Unavailable,
            "{source}"
        );
    }
    assert_value("true || fromJSON('bad')", json!(true));
    assert_value("false && fromJSON('bad')", json!(false));
    let mut budget = EvaluationBudget::default();
    let selected = traced(
        "data.flag && data.used",
        &json!({"data": {"flag": true, "used": 1, "unused": 2}}),
        &EvaluationBindings::new(),
        &mut budget,
    );
    assert!(
        selected
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == path(&["data", "flag"]))
    );
    assert!(
        selected
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == path(&["data", "used"]))
    );
    assert!(
        !selected
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == path(&["data", "unused"]))
    );
}

#[test]
fn comparisons_keep_numeric_precision_and_convert_only_different_types() {
    for (source, expected) in [
        ("9007199254740993 == 9007199254740992.0", false),
        ("9007199254740993 > 9007199254740992.0", true),
        ("'9007199254740993' == 9007199254740993", true),
        ("18446744073709551615 > 18446744073709551614", true),
        ("'2' < 3", true),
        ("'2' < '10'", false),
        ("false < '1'", true),
        ("null <= 0", true),
        ("'' == false", true),
        ("'0x10' == 16", false),
        ("' 1 ' == 1", true),
        ("'bad' > 0", false),
        ("'bad' != 0", true),
        ("'İ' == 'i̇'", true),
    ] {
        assert_value(source, json!(expected));
    }
}

#[test]
fn unicode_lowercase_preserves_contextual_final_sigma() {
    for (source, expected) in [
        ("'ΟΣ' == 'ος'", true),
        ("'ΟΣ' != 'ος'", false),
        ("'ΟΣ' < 'ος'", false),
        ("'ΟΣ' <= 'ος'", true),
        ("'ΟΣ' > 'ος'", false),
        ("'ΟΣ' >= 'ος'", true),
        ("'ΟΣ' < 'οσ'", true),
        ("'οσ' > 'ΟΣ'", true),
        ("'ΟΣΑ' == 'οσα'", true),
        ("'ΟΣ Α' == 'ος α'", true),
        ("'ΟΣ\u{0301}' == 'ος\u{0301}'", true),
        ("contains('prefix ΟΣ suffix', 'ος')", true),
        ("startsWith('ΟΣ suffix', 'ος')", true),
        ("endsWith('prefix ΟΣ', 'ος')", true),
        ("contains(fromJSON('[\"ΟΣ\"]'), 'ος')", true),
        ("contains('ΟΣΑ', 'ΟΣ')", false),
        ("startsWith('ΟΣΑ', 'ΟΣ')", false),
        ("endsWith('ΟΣΑ', 'ΟΣ')", false),
        ("startsWith('ΟΣΑ', 'οσ')", true),
    ] {
        assert_value(source, json!(expected));
    }
}

#[test]
fn unicode_lowercase_reserves_exact_generated_bytes_before_allocating() {
    for (upper, lower, comparison_bytes, text_bytes) in [
        ("ΟΣ", "ος", 8, 16),
        ("İ", "i\u{0307}", 6, 11),
        ("K", "k", 2, 6),
    ] {
        let context = json!({"upper": upper, "lower": lower, "items": [upper]});
        for (source, expected, bytes) in [
            ("upper == lower", true, comparison_bytes),
            ("upper != lower", false, comparison_bytes),
            ("upper < lower", false, comparison_bytes),
            ("upper <= lower", true, comparison_bytes),
            ("upper > lower", false, comparison_bytes),
            ("upper >= lower", true, comparison_bytes),
            ("contains(items, lower)", true, comparison_bytes),
            ("contains(upper, lower)", true, text_bytes),
            ("startsWith(upper, lower)", true, text_bytes),
            ("endsWith(upper, lower)", true, text_bytes),
        ] {
            let expression = parse_expression(source).unwrap();
            for fits in [true, false] {
                let mut budget = EvaluationBudget::default();
                budget
                    .reserve_generated_bytes(
                        DEFAULT_MAX_GENERATED_BYTES - bytes + usize::from(!fits),
                    )
                    .unwrap();
                let result = evaluate_expression_with_budget(
                    &expression,
                    &context,
                    &EvaluationBindings::new(),
                    &mut budget,
                    1,
                );
                if fits {
                    assert_eq!(
                        result.unwrap().value,
                        EvaluationValue::Available(json!(expected)),
                        "{source}: {upper}"
                    );
                    assert_eq!(budget.generated_bytes(), DEFAULT_MAX_GENERATED_BYTES);
                    assert!(!budget.exhausted());
                } else {
                    assert_eq!(
                        result.unwrap_err().code,
                        DiagnosticCode::EvaluationLimitExceeded,
                        "{source}: {upper}"
                    );
                    assert!(budget.exhausted());
                    assert!(budget.generated_bytes() <= DEFAULT_MAX_GENERATED_BYTES);
                    assert!(budget.reserve_generated_bytes(0).is_err());
                }
            }
        }
    }
}

#[test]
fn unicode_comparison_budget_failure_discards_earlier_status_actions() {
    let context = json!({"message": {"tool": {"arguments": {
        "upper": "ΟΣ", "lower": "ος", "items": ["ΟΣ"]
    }}}});
    for condition in [
        "message.tool.arguments.upper == message.tool.arguments.lower",
        "message.tool.arguments.upper <= message.tool.arguments.lower",
        "contains(message.tool.arguments.items, message.tool.arguments.lower)",
    ] {
        let check = crate::compiler::compile(&format!(
            "name: Unicode budget\non: {{tool_result: null}}\nactions:\n  - type: status\n    input: {{result: success}}\n  - type: status\n    if: {condition}\n    input: {{result: failure}}\n"
        ))
        .unwrap();
        let mut budget = EvaluationBudget::default();
        budget
            .reserve_generated_bytes(DEFAULT_MAX_GENERATED_BYTES - 7)
            .unwrap();
        let error = crate::actions::plan_status_actions_with_budget(
            &check.actions,
            &context,
            &crate::results::EvaluatedAssessment::local(),
            None,
            &mut budget,
        )
        .unwrap_err();
        assert_eq!(error.code, DiagnosticCode::EvaluationLimitExceeded);
        assert!(budget.exhausted());
    }
}

#[test]
fn strict_equals_keeps_source_strings_case_sensitive_without_lowercase_allocation() {
    for (upper, lower) in [("ΟΣ", "ος"), ("İ", "i\u{0307}"), ("K", "k")] {
        let context = json!({"upper": upper, "lower": lower});
        for (source, expected) in [
            ("strictEquals(upper, lower)", false),
            ("strictEquals(lower, lower)", true),
        ] {
            let mut budget = EvaluationBudget::new(EvaluationLimits::default(), 1000, 0);
            let value = traced(source, &context, &EvaluationBindings::new(), &mut budget);
            assert_eq!(value.value, EvaluationValue::Available(json!(expected)));
            assert_eq!(budget.generated_bytes(), 0);
            assert!(!budget.exhausted());
        }
    }
}

#[test]
fn aliases_preserve_frozen_identity_but_json_roundtrips_do_not() {
    let context = json!({"data": [{"keep": true, "used": {"x": 1}, "unused": 5}, {"keep": false, "unused": 9}]});
    let mut budget = EvaluationBudget::default();
    let collection = traced("data", &context, &EvaluationBindings::new(), &mut budget);
    let item = collection.array_item(0, &mut budget).unwrap();
    let bindings = EvaluationBindings::from([("row".to_owned(), item)]);
    for (source, expected) in [
        ("row == data[0]", true),
        ("row.used == data[0].used", true),
        ("contains(data, row)", true),
        ("contains(data.*, row)", true),
        ("fromJSON(toJSON(row)) == row", false),
        ("fromJSON(toJSON(row)).used == row.used", false),
        ("contains(data, fromJSON(toJSON(row)))", false),
    ] {
        assert_eq!(
            traced(source, &context, &bindings, &mut budget).value,
            EvaluationValue::Available(json!(expected)),
            "{source}"
        );
    }
    let copied = traced(
        "fromJSON(toJSON(row.used))",
        &context,
        &bindings,
        &mut budget,
    );
    let bindings = EvaluationBindings::from([("copied".to_owned(), copied)]);
    assert_eq!(
        traced("copied == copied", &context, &bindings, &mut budget).value,
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        traced("copied == data[0].used", &context, &bindings, &mut budget).value,
        EvaluationValue::Available(json!(false))
    );
}

#[test]
fn map_style_traces_do_not_select_excluded_unused_fields() {
    let context =
        json!({"data": [{"keep": true, "used": 1, "unused": 5}, {"keep": false, "unused": 9}]});
    let mut budget = EvaluationBudget::default();
    let collection = traced("data", &context, &EvaluationBindings::new(), &mut budget);
    let mut trace = collection.membership_trace();
    for index in 0..2 {
        let item = collection.array_item(index, &mut budget).unwrap();
        let bindings = EvaluationBindings::from([("row".to_owned(), item)]);
        let condition = traced("row.keep", &context, &bindings, &mut budget);
        trace.merge(&condition.trace);
        if condition.value == EvaluationValue::Available(json!(true)) {
            trace.merge(
                &traced(
                    "fromJSON(toJSON(row.used))",
                    &context,
                    &bindings,
                    &mut budget,
                )
                .trace,
            );
        }
    }
    assert_eq!(trace.dependencies.len(), 4);
    assert!(
        trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == path(&["data"])
                && dependency.kind == EvaluationDependencyKind::Membership)
    );
    for dependency in trace.dependencies {
        assert!(
            !dependency
                .path
                .iter()
                .any(|segment| matches!(segment, PathSegment::Property(name) if name == "unused"))
        );
        assert!(
            !(dependency.path == path(&["data"])
                && dependency.kind == EvaluationDependencyKind::Value)
        );
    }
}

#[test]
fn projection_item_aliases_keep_original_source_identity_and_indexes() {
    let context = json!({"data": [{"nested": {"value": 1}}, {"nested": {"value": 2}}]});
    let mut budget = EvaluationBudget::default();
    let collection = traced(
        "data.*.nested",
        &context,
        &EvaluationBindings::new(),
        &mut budget,
    );
    let item = collection.array_item(1, &mut budget).unwrap();
    let bindings = EvaluationBindings::from([("row".to_owned(), item)]);
    let selected = traced("row.value", &context, &bindings, &mut budget);
    assert!(
        selected
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path
                == vec![
                    PathSegment::Property("data".to_owned()),
                    PathSegment::Index(1),
                    PathSegment::Property("nested".to_owned()),
                    PathSegment::Property("value".to_owned())
                ])
    );
    assert_eq!(
        traced("row == data[1].nested", &context, &bindings, &mut budget).value,
        EvaluationValue::Available(json!(true))
    );
    assert!(
        !selected
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path.contains(&PathSegment::Index(0)))
    );
}

#[test]
fn conversions_keep_full_dependency_when_reading_only_one_parsed_field() {
    let context = json!({"data": {"x": 1, "y": 2}});
    let mut budget = EvaluationBudget::default();
    let value = traced(
        "fromJSON(toJSON(data)).x",
        &context,
        &EvaluationBindings::new(),
        &mut budget,
    );
    assert_eq!(
        value.trace.dependencies,
        vec![EvaluationDependency {
            path: path(&["data"]),
            kind: EvaluationDependencyKind::Value
        }]
    );
}

#[test]
fn budgets_are_shared_sticky_and_serializable_without_counter_reset() {
    let limits = EvaluationLimits {
        max_depth: 32,
        max_steps: 5,
    };
    let mut budget = EvaluationBudget::new(limits, 1000, 1024);
    let expression = parse_expression("true").unwrap();
    for _ in 0..5 {
        evaluate_expression_with_budget(
            &expression,
            &json!({}),
            &EvaluationBindings::new(),
            &mut budget,
            1,
        )
        .unwrap();
    }
    let mut restored: EvaluationBudget =
        serde_json::from_value(serde_json::to_value(&budget).unwrap()).unwrap();
    assert_eq!(restored, budget);
    assert_eq!(
        evaluate_expression_with_budget(
            &expression,
            &json!({}),
            &EvaluationBindings::new(),
            &mut restored,
            1
        )
        .unwrap_err()
        .code,
        DiagnosticCode::EvaluationLimitExceeded
    );
    assert!(restored.exhausted());
    assert!(restored.reserve_generated_bytes(0).is_err());
    assert!(restored.visit_items(0).is_err());
    assert!(restored.validate_for_resume().is_err());
    let mut saved = serde_json::to_value(EvaluationBudget::default()).unwrap();
    saved["steps"] = json!(10_001);
    assert!(serde_json::from_value::<EvaluationBudget>(saved).is_err());
    let mut default = EvaluationBudget::default();
    default.step(1).unwrap();
    let restored: EvaluationBudget =
        serde_json::from_value(serde_json::to_value(&default).unwrap()).unwrap();
    restored.validate_for_resume().unwrap();
    assert_eq!(restored.steps(), 1);
    assert!(budget.validate_for_resume().is_err());
}

#[test]
fn budget_limits_stop_generated_text_json_iteration_and_nested_depth() {
    for (source, context, max_items, max_bytes) in [
        ("toJSON(data)", json!({"data": "x".repeat(100)}), 1000, 10),
        ("data.*", json!({"data": [1, 2, 3]}), 2, 1024),
        ("fromJSON(data)", json!({"data": "[1,2,3]"}), 2, 1024),
        (
            "format('{0}{0}{0}', data)",
            json!({"data": "abcd"}),
            1000,
            15,
        ),
        ("join(data, '--')", json!({"data": [1, 2, 3]}), 1000, 10),
    ] {
        let mut budget = EvaluationBudget::new(EvaluationLimits::default(), max_items, max_bytes);
        let error = evaluate_expression_with_budget(
            &parse_expression(source).unwrap(),
            &context,
            &EvaluationBindings::new(),
            &mut budget,
            1,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            DiagnosticCode::EvaluationLimitExceeded,
            "{source}"
        );
        assert!(budget.exhausted());
        assert!(budget.generated_bytes() <= max_bytes);
        assert!(budget.visited_items() <= max_items);
    }
    let mut budget = EvaluationBudget::default();
    assert_eq!(
        evaluate_expression_with_budget(
            &parse_expression("!true").unwrap(),
            &json!({}),
            &EvaluationBindings::new(),
            &mut budget,
            32
        )
        .unwrap_err()
        .code,
        DiagnosticCode::EvaluationLimitExceeded
    );
    let mut budget = EvaluationBudget::default();
    let text = format!("{}0{}", "[".repeat(33), "]".repeat(33));
    assert_eq!(
        evaluate_expression_with_budget(
            &parse_expression("fromJSON(data)").unwrap(),
            &json!({"data": text}),
            &EvaluationBindings::new(),
            &mut budget,
            1
        )
        .unwrap_err()
        .code,
        DiagnosticCode::EvaluationLimitExceeded
    );
}

#[test]
fn text_conversion_never_re_evaluates_source_expressions() {
    let text = "${{ unavailable.path }}";
    let context = json!({"data": text});
    assert_eq!(
        eval("format('{0}', data)", &context),
        EvaluationValue::Available(json!(text))
    );
    assert_eq!(
        scalar_text(&Value::Null, &mut EvaluationBudget::default()).unwrap(),
        ""
    );
    assert!(scalar_text(&json!([]), &mut EvaluationBudget::default()).is_err());
}

#[test]
fn compact_json_amplification_stops_at_the_shared_item_bound() {
    for text in [
        format!("[{}]", vec!["0"; 50_000].join(",")),
        format!(
            "{{{}}}",
            (0..50_000)
                .map(|index| format!("\"k{index}\":0"))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ] {
        let mut budget = EvaluationBudget::default();
        let error = evaluate_expression_with_budget(
            &parse_expression("fromJSON(data)").unwrap(),
            &json!({"data": text}),
            &EvaluationBindings::new(),
            &mut budget,
            1,
        )
        .unwrap_err();
        assert_eq!(error.code, DiagnosticCode::EvaluationLimitExceeded);
        assert_eq!(budget.visited_items(), DEFAULT_MAX_EVALUATION_ITEMS);
        assert!(budget.steps() < 2_000);
        assert!(budget.exhausted());
    }
}

#[test]
fn many_small_function_leaves_cannot_reset_generated_byte_costs() {
    let expression = parse_expression("format('{0}{0}', data)").unwrap();
    let context = json!({"data": "small"});
    let mut budget = EvaluationBudget::new(EvaluationLimits::default(), 1000, 128);
    let mut completed = 0;
    loop {
        match evaluate_expression_with_budget(
            &expression,
            &context,
            &EvaluationBindings::new(),
            &mut budget,
            1,
        ) {
            Ok(_) => completed += 1,
            Err(error) => {
                assert_eq!(error.code, DiagnosticCode::EvaluationLimitExceeded);
                break;
            }
        }
    }
    assert!(completed > 1 && completed < 20);
    assert!(budget.exhausted());
    assert!(budget.generated_bytes() <= 128);
    let mut saved = serde_json::to_value(EvaluationBudget::default()).unwrap();
    saved["limits"]["max_steps"] = json!(usize::MAX);
    assert!(serde_json::from_value::<EvaluationBudget>(saved).is_err());
}

#[test]
fn raw_source_copy_cost_does_not_consume_generated_byte_allowance() {
    let mut budget = EvaluationBudget::new(EvaluationLimits::default(), 1000, 0);
    let context = json!({"data": "raw source text"});
    let value = traced("data", &context, &EvaluationBindings::new(), &mut budget);
    assert_eq!(
        value.value,
        EvaluationValue::Available(context["data"].clone())
    );
    assert_eq!(budget.generated_bytes(), 0);
    assert_eq!(budget.materialized_bytes(), "raw source text".len());
    assert!(!budget.exhausted());
}

#[test]
fn repeated_source_leaves_share_the_materialization_limit_before_cloning() {
    let context = json!({"data": "abcd"});
    for source in ["data", "join(data)", "false || data", "true && data"] {
        let mut budget = EvaluationBudget::new_with_materialization_limit(
            EvaluationLimits::default(),
            1000,
            1024,
            10,
        );
        let expression = parse_expression(source).unwrap();
        let mut retained = Vec::new();
        for _ in 0..2 {
            retained.push(
                evaluate_expression_with_budget(
                    &expression,
                    &context,
                    &EvaluationBindings::new(),
                    &mut budget,
                    1,
                )
                .unwrap(),
            );
        }
        assert_eq!(budget.materialized_bytes(), 8, "{source}");
        let error = evaluate_expression_with_budget(
            &expression,
            &context,
            &EvaluationBindings::new(),
            &mut budget,
            1,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            DiagnosticCode::EvaluationLimitExceeded,
            "{source}"
        );
        assert!(
            error.message.contains("materialized-byte limit"),
            "{source}"
        );
        assert_eq!(budget.materialized_bytes(), 8, "{source}");
        assert_eq!(retained.len(), 2);
        assert!(budget.exhausted());
        assert!(budget.reserve_materialized_bytes(0).is_err());
        assert!(budget.reserve_generated_bytes(0).is_err());
    }
}

#[test]
fn map_alias_item_and_field_copies_share_materialization_costs() {
    let context = json!({"data": ["abcd"]});
    let mut budget = EvaluationBudget::new_with_materialization_limit(
        EvaluationLimits::default(),
        1000,
        1024,
        21,
    );
    let collection = traced("data", &context, &EvaluationBindings::new(), &mut budget);
    assert_eq!(budget.materialized_bytes(), 7);
    let item = collection.array_item(0, &mut budget).unwrap();
    assert_eq!(budget.materialized_bytes(), 11);
    let bindings = EvaluationBindings::from([("row".to_owned(), item)]);
    let first = traced("row", &context, &bindings, &mut budget);
    let second = traced("row", &context, &bindings, &mut budget);
    assert_eq!(first.value, EvaluationValue::Available(json!("abcd")));
    assert_eq!(second.value, first.value);
    assert_eq!(budget.materialized_bytes(), 19);
    let error = collection.array_item(0, &mut budget).unwrap_err();
    assert_eq!(error.code, DiagnosticCode::EvaluationLimitExceeded);
    assert_eq!(budget.materialized_bytes(), 19);
    assert!(budget.exhausted());
}

#[test]
fn projection_branches_share_the_materialization_limit_across_evaluations() {
    let context = json!({"data": [{"value": "abcd"}, {"value": "efgh"}]});
    let mut budget = EvaluationBudget::new_with_materialization_limit(
        EvaluationLimits::default(),
        1000,
        1024,
        12,
    );
    let expression = parse_expression("data.*.value").unwrap();
    let retained = traced(
        "data.*.value",
        &context,
        &EvaluationBindings::new(),
        &mut budget,
    );
    assert_eq!(
        retained.value,
        EvaluationValue::Available(json!(["abcd", "efgh"]))
    );
    assert_eq!(budget.materialized_bytes(), 8);
    let error = evaluate_expression_with_budget(
        &expression,
        &context,
        &EvaluationBindings::new(),
        &mut budget,
        1,
    )
    .unwrap_err();
    assert_eq!(error.code, DiagnosticCode::EvaluationLimitExceeded);
    assert_eq!(budget.materialized_bytes(), 12);
    assert!(budget.exhausted());
}

#[test]
fn generated_and_projected_aliases_cannot_reset_materialization_costs() {
    let context = json!({"data": [{"value": "abcd"}], "text": "[\"abcd\"]"});
    for source in ["data.*.value", "fromJSON(text)"] {
        let mut budget = EvaluationBudget::new_with_materialization_limit(
            EvaluationLimits::default(),
            1000,
            1024,
            12,
        );
        let collection = traced(source, &context, &EvaluationBindings::new(), &mut budget);
        let initial_materialized = budget.materialized_bytes();
        let item = collection.array_item(0, &mut budget).unwrap();
        assert_eq!(budget.materialized_bytes(), initial_materialized + 4);
        let bindings = EvaluationBindings::from([
            ("collection".to_owned(), collection),
            ("row".to_owned(), item),
        ]);
        let selected = traced("collection[0]", &context, &bindings, &mut budget);
        assert_eq!(selected.value, EvaluationValue::Available(json!("abcd")));
        if initial_materialized == 0 {
            let selected = traced("row", &context, &bindings, &mut budget);
            assert_eq!(selected.value, EvaluationValue::Available(json!("abcd")));
        }
        assert_eq!(budget.materialized_bytes(), 12, "{source}");
        let error = evaluate_expression_with_budget(
            &parse_expression("row").unwrap(),
            &context,
            &bindings,
            &mut budget,
            1,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            DiagnosticCode::EvaluationLimitExceeded,
            "{source}"
        );
        assert_eq!(budget.materialized_bytes(), 12, "{source}");
        assert!(budget.exhausted());
    }
}

#[test]
fn materialization_counters_survive_budget_clone_and_restart() {
    let context = json!({"data": "abcd"});
    let mut budget = EvaluationBudget::new_with_materialization_limit(
        EvaluationLimits::default(),
        1000,
        1024,
        10,
    );
    let retained = traced("data", &context, &EvaluationBindings::new(), &mut budget);
    let cloned = budget.clone();
    assert_eq!(cloned.materialized_bytes(), 4);
    let mut restored: EvaluationBudget =
        serde_json::from_value(serde_json::to_value(&cloned).unwrap()).unwrap();
    assert_eq!(restored, budget);
    let second = traced("data", &context, &EvaluationBindings::new(), &mut restored);
    assert_eq!(second.value, retained.value);
    assert_eq!(restored.materialized_bytes(), 8);
    let error = evaluate_expression_with_budget(
        &parse_expression("data").unwrap(),
        &context,
        &EvaluationBindings::new(),
        &mut restored,
        1,
    )
    .unwrap_err();
    assert_eq!(error.code, DiagnosticCode::EvaluationLimitExceeded);
    assert_eq!(restored.materialized_bytes(), 8);
    assert!(restored.exhausted());
    let restored: EvaluationBudget =
        serde_json::from_value(serde_json::to_value(&restored).unwrap()).unwrap();
    assert!(restored.exhausted());
    assert!(restored.validate_for_resume().is_err());

    let mut default = EvaluationBudget::default();
    default
        .reserve_materialized_bytes(DEFAULT_MAX_MATERIALIZED_BYTES - 4)
        .unwrap();
    let mut restored: EvaluationBudget =
        serde_json::from_value(serde_json::to_value(&default).unwrap()).unwrap();
    restored.validate_for_resume().unwrap();
    traced("data", &context, &EvaluationBindings::new(), &mut restored);
    assert_eq!(
        restored.materialized_bytes(),
        DEFAULT_MAX_MATERIALIZED_BYTES
    );
    assert!(restored.reserve_materialized_bytes(1).is_err());
    assert!(restored.exhausted());
}

#[test]
fn saved_materialization_limits_and_counters_cannot_bypass_deployed_bounds() {
    for (field, value) in [
        ("materialized_bytes", DEFAULT_MAX_MATERIALIZED_BYTES + 1),
        ("max_materialized_bytes", DEFAULT_MAX_MATERIALIZED_BYTES + 1),
        ("max_materialized_bytes", usize::MAX),
    ] {
        let mut saved = serde_json::to_value(EvaluationBudget::default()).unwrap();
        saved[field] = json!(value);
        assert!(
            serde_json::from_value::<EvaluationBudget>(saved).is_err(),
            "{field}"
        );
    }
    for field in ["materialized_bytes", "max_materialized_bytes"] {
        let mut saved = serde_json::to_value(EvaluationBudget::default()).unwrap();
        saved.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<EvaluationBudget>(saved).is_err(),
            "{field}"
        );
    }
    let mut budget = EvaluationBudget::new_with_materialization_limit(
        EvaluationLimits::default(),
        1000,
        1024,
        10,
    );
    assert!(budget.validate_for_resume().is_err());
    assert!(budget.reserve_materialized_bytes(usize::MAX).is_err());
    assert_eq!(budget.materialized_bytes(), 0);
    assert!(budget.exhausted());
}

#[test]
fn json_roundtrips_keep_precise_collection_membership_without_changing_value_traces() {
    let context = json!({"session": {"id": "s1", "requests": [{"text": "known"}], "requests_complete": false}});
    let request_path = path(&["session", "requests"]);
    for expression in [
        "fromJSON(toJSON(session.requests)).length",
        "fromJSON(toJSON(session.requests))[0].text",
        "fromJSON(toJSON(session.requests)).*.text",
        "fromJSON(toJSON(session)).requests.length",
        "fromJSON(toJSON(session)).requests[0].text",
        "fromJSON(toJSON(session)).requests.*.text",
        "fromJSON(toJSON(fromJSON(toJSON(session)))).requests.length",
        "contains(fromJSON(toJSON(session.requests)), null)",
    ] {
        let value = traced(
            expression,
            &context,
            &EvaluationBindings::new(),
            &mut EvaluationBudget::default(),
        );
        assert!(
            value
                .trace
                .dependencies
                .iter()
                .any(|dependency| dependency.path == request_path
                    && dependency.kind == EvaluationDependencyKind::Membership),
            "{expression}: {:?}",
            value.trace
        );
    }
    for expression in [
        "session.requests",
        "session",
        "toJSON(session.requests)",
        "toJSON(session)",
        "fromJSON(toJSON(session.requests))",
        "fromJSON(toJSON(session))",
        "fromJSON(toJSON(session)).requests",
        "fromJSON(toJSON(session)).id",
    ] {
        let value = traced(
            expression,
            &context,
            &EvaluationBindings::new(),
            &mut EvaluationBudget::default(),
        );
        assert!(
            value
                .trace
                .dependencies
                .iter()
                .all(|dependency| dependency.kind == EvaluationDependencyKind::Value),
            "{expression}: {:?}",
            value.trace
        );
    }
    for expression in [
        "fromJSON(toJSON(session.requests))",
        "fromJSON(toJSON(session)).requests",
    ] {
        let value = traced(
            expression,
            &context,
            &EvaluationBindings::new(),
            &mut EvaluationBudget::default(),
        );
        assert!(
            value
                .membership_trace()
                .dependencies
                .iter()
                .any(|dependency| dependency.path == request_path
                    && dependency.kind == EvaluationDependencyKind::Membership),
            "{expression}"
        );
    }
}

#[test]
fn converted_projection_aliases_keep_source_shape_but_get_fresh_identity() {
    let context = json!({"groups": [{"requests": []}, {"requests": [{"text": "known"}]}]});
    let selected_path = vec![
        PathSegment::Property("groups".to_owned()),
        PathSegment::Index(1),
        PathSegment::Property("requests".to_owned()),
    ];
    let unused_path = vec![
        PathSegment::Property("groups".to_owned()),
        PathSegment::Index(0),
        PathSegment::Property("requests".to_owned()),
    ];
    let mut budget = EvaluationBudget::default();
    let collection = traced(
        "fromJSON(toJSON(groups.*.requests))",
        &context,
        &EvaluationBindings::new(),
        &mut budget,
    );
    let item = collection.array_item(1, &mut budget).unwrap();
    let bindings = EvaluationBindings::from([("row".to_owned(), item)]);
    let value = traced("row.length", &context, &bindings, &mut budget);
    assert_eq!(value.value, EvaluationValue::Available(json!(1)));
    assert!(
        value
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == selected_path
                && dependency.kind == EvaluationDependencyKind::Membership)
    );
    assert!(
        !value
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == unused_path
                && dependency.kind == EvaluationDependencyKind::Membership)
    );
    assert_eq!(
        traced("row == row", &context, &bindings, &mut budget).value,
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        traced(
            "row == groups[1].requests",
            &context,
            &bindings,
            &mut budget
        )
        .value,
        EvaluationValue::Available(json!(false))
    );

    let value = traced(
        "fromJSON(toJSON(groups.*.requests))[1].length",
        &context,
        &EvaluationBindings::new(),
        &mut budget,
    );
    assert!(
        value
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == selected_path
                && dependency.kind == EvaluationDependencyKind::Membership)
    );
    assert!(
        !value
            .trace
            .dependencies
            .iter()
            .any(|dependency| dependency.path == unused_path
                && dependency.kind == EvaluationDependencyKind::Membership)
    );
}

#[test]
fn converted_open_json_and_window_collections_do_not_select_request_membership() {
    let context = json!({
        "session": {"requests": [], "requests_complete": false},
        "message": {"tool": {"arguments": {"items": []}, "result": {"content": "[]"}}},
        "window": {"messages": []},
    });
    for expression in [
        "fromJSON(toJSON(message.tool.arguments)).items.length",
        "fromJSON(toJSON(window)).messages.length",
        "fromJSON(toJSON(window.messages)).length",
        "fromJSON(message.tool.result.content).length",
        "fromJSON('[]').length",
    ] {
        let value = traced(
            expression,
            &context,
            &EvaluationBindings::new(),
            &mut EvaluationBudget::default(),
        );
        assert_eq!(
            value.value,
            EvaluationValue::Available(json!(0)),
            "{expression}"
        );
        assert!(
            !value
                .trace
                .dependencies
                .iter()
                .any(
                    |dependency| dependency.path == path(&["session", "requests"])
                        && dependency.kind == EvaluationDependencyKind::Membership
                ),
            "{expression}"
        );
    }
    for expression in [
        "fromJSON(toJSON(session)).requests == session.requests",
        "fromJSON(toJSON(session.requests)) == session.requests",
    ] {
        assert_eq!(
            eval(expression, &context),
            EvaluationValue::Available(json!(false)),
            "{expression}"
        );
    }
}
