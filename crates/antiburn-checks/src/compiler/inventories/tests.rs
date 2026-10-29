use super::*;
use crate::compiler::{InputLiteral, compile};
use crate::expression::parse_expression;

fn expression(source: &str) -> Expression {
    parse_expression(source).unwrap()
}

fn check() -> CompiledCheck {
    compile("name: Inventory\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap()
}

#[test]
fn inventory_demand_covers_conditions_actions_and_computed_scopes() {
    for source in [
        "global.instructions",
        "repo.skills.length",
        "toJSON(repo)",
        "(repo)['instructions']",
        "repo[fromJSON('\"skills\"')].length",
        "message[fromJSON(toJSON(global.skills))]",
    ] {
        let mut check = check();
        check.condition = Some(Condition::Expression(expression(source)));
        assert!(inspect(&check).condition, "{source}");
        assert!(!inspect(&check).candidate);
        check.condition = None;
        check.actions[0].result = StatusResultTemplate::Expression(expression(source));
        assert!(inspect(&check).candidate, "{source}");
        assert!(!inspect(&check).condition);
    }
}

#[test]
fn inventory_demand_does_not_capture_for_recorded_fields_or_repo_root() {
    for source in [
        "message.tool.result.outcome.exit",
        "session.requests",
        "repo.root",
        "(repo).root",
        "repo['root']",
        "agent.skills",
    ] {
        assert!(!expression_reads(&expression(source), &[]), "{source}");
    }
}

#[test]
fn inventory_demand_handles_nested_templates_and_shadowed_aliases() {
    let mut check = check();
    let mut template = InputTemplate::Map {
        collection: expression("session.requests"),
        alias: "repo".into(),
        condition: Some(Condition::Expression(expression("repo.instructions != ''"))),
        value: Box::new(InputTemplate::Object(vec![(
            "nested".into(),
            InputTemplate::Sequence(vec![
                InputTemplate::Text(vec![TemplatePart::Expression(expression("repo.skills"))]),
                InputTemplate::Literal(InputLiteral::Null),
            ]),
        )])),
    };
    check.actions[0].message = Some(template.clone());
    assert_eq!(inspect(&check), CurrentInventoryDemand::default());
    if let InputTemplate::Map { value, .. } = &mut template {
        **value = InputTemplate::Map {
            collection: expression("global.skills"),
            alias: "global".into(),
            condition: Some(Condition::Expression(expression("global.name != ''"))),
            value: Box::new(InputTemplate::Expression(expression("global.description"))),
        };
    }
    check.actions[0].message = Some(template);
    assert!(inspect(&check).candidate);
}

#[test]
fn inventory_demand_reads_map_conditions_and_dynamic_keys() {
    for template in [
        InputTemplate::Map {
            collection: expression("session.requests"),
            alias: "item".into(),
            condition: Some(Condition::Expression(expression(
                "global.instructions_complete",
            ))),
            value: Box::new(InputTemplate::Expression(expression("item.text"))),
        },
        InputTemplate::Expression(expression("repo['root'][toJSON(global.skills)]")),
    ] {
        assert!(template_reads(&template, &mut Vec::new()));
    }
}

#[test]
fn fast_mode_evidence_is_an_admitted_quality_member_and_projection() {
    for access in [
        "message.quality.fast_mode_evidence",
        "message.quality['fast_mode_evidence']",
        "toJSON(message.quality.*)",
    ] {
        let source = format!(
            "name: Qualifier\nif: {access} == 'reported-speed'\non:\n  tool_result:\nactions:\n  - type: status\n    input: {{result: failure}}\n"
        );
        let check = compile(&source).unwrap();
        assert_eq!(inspect(&check), CurrentInventoryDemand::default());
    }
}

#[test]
fn inventory_demand_follows_compiled_nested_input_aliases() {
    let source = r#"name: Nested inventory
on:
  tool_result:
decisions:
  review:
    input:
      nested:
        - map: "${{ session.requests }}"
          as: request
          if: request.text != ''
          value:
            map: "${{ global.skills }}"
            as: skill
            if: skill.name != ''
            value:
              name: "${{ skill.name }}"
              text: "${{ request.text }}"
    output:
      useful:
        type: noul
        question: Is this useful?
actions:
  - type: status
    input: {result: failure}
"#;
    let check = compile(source).unwrap();
    assert_eq!(
        inspect(&check),
        CurrentInventoryDemand {
            condition: false,
            candidate: true
        }
    );
}
