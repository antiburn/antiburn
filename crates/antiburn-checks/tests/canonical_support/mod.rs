use antiburn_checks::CompiledCheck;
use antiburn_checks::context::CurrentInventories;
use antiburn_checks::expression_eval::{EvaluationBudget, EvaluationValue};
use antiburn_checks::preparation::evaluate_decision_input_with_budget;
use serde_json::{Value, json};

pub const DEFINITIONS: [(&str, &str); 5] = [
    (
        "over-exploring",
        include_str!("../../../../.antiburn/checks/over-exploring.yml"),
    ),
    (
        "scope-creep",
        include_str!("../../../../.antiburn/checks/scope-creep.yml"),
    ),
    (
        "ignored-instructions",
        include_str!("../../../../.antiburn/checks/ignored-instructions.yml"),
    ),
    (
        "skill-opportunities",
        include_str!("../../../../.antiburn/checks/skill-opportunities.yml"),
    ),
    (
        "fast-mode-value",
        include_str!("../../../../.antiburn/checks/fast-mode-value.yml"),
    ),
];

pub fn check(id: &str) -> CompiledCheck {
    antiburn_checks::compile(DEFINITIONS.iter().find(|(name, _)| *name == id).unwrap().1).unwrap()
}

pub fn context(check: &CompiledCheck, request: &str, result: &str) -> Value {
    let inventories = serde_json::to_value(CurrentInventories::default()).unwrap();
    let messages = (0..check.trigger.window.unwrap().minimum)
        .map(|index| {
            json!({
                "id": format!("result-{index}"), "source": "synthetic", "kind": "tool_result",
                "native_order": format!("native:{index}"), "fast_mode": true,
                "tool": {
                    "name": "read", "category": "read", "arguments": {"path": "src/parser.rs"},
                    "result": {"content": result, "status": "completed"}
                },
                "quality": {"source_complete": true, "request_history_complete": true}
            })
        })
        .collect::<Vec<_>>();
    json!({
        "session": {
            "requests": [{"id": "request-1", "source": "synthetic", "kind": "user_message", "text": request}],
            "requests_complete": true, "source_coverage": "complete"
        },
        "window": {"messages": messages},
        "global": inventories["global"], "repo": inventories["repo"]
    })
}

pub fn input(check: &CompiledCheck, context: &Value) -> Value {
    let evaluated = evaluate_decision_input_with_budget(
        &check.decisions[0],
        context,
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    let EvaluationValue::Available(value) = evaluated.value else {
        panic!("{} input must remain available", check.name)
    };
    value
}
