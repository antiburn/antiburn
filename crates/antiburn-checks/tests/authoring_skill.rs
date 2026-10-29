use antiburn_checks::actions::plan_status_actions;
use antiburn_checks::compile;
use antiburn_checks::compiler::StatusResult;
use antiburn_checks::results::EvaluatedAssessment;
use serde_json::json;

fn yaml_example(skill: &str) -> Option<String> {
    let mut lines = skill.lines();
    lines.find(|line| *line == "```yaml")?;
    let mut source = Vec::new();
    for line in lines {
        if line == "```" {
            return Some(source.join("\n"));
        }
        source.push(line);
    }
    None
}

#[test]
fn yaml_example_extraction_accepts_lf_and_crlf() {
    for newline in ["\n", "\r\n"] {
        let skill = [
            "# Skill",
            "```yaml",
            "name: Example",
            "on:",
            "  tool_result:",
            "```",
            "After.",
        ]
        .join(newline);
        assert_eq!(
            yaml_example(&skill).as_deref(),
            Some("name: Example\non:\n  tool_result:")
        );
    }
}

#[test]
fn yaml_example_extraction_requires_complete_fences() {
    for newline in ["\n", "\r\n"] {
        for lines in [["name: Example", "```"], ["```yaml", "name: Example"]] {
            assert_eq!(yaml_example(&lines.join(newline)), None);
        }
    }
}

#[test]
fn installed_skill_example_compiles_and_keeps_its_stated_result_policy() {
    let skill =
        include_str!("../../../.agents/skills/antiburn-checks/SKILL.md").replace("\r\n", "\n");
    for newline in ["\n", "\r\n"] {
        let source = yaml_example(&skill.replace('\n', newline))
            .expect("The installed skill must contain a complete YAML example.");
        let check = compile(&source).unwrap();
        assert!(check.is_local());
        assert!(check.question_requirements().is_empty());
        for (status, prior, expected) in [
            ("cancelled", None, Some(StatusResult::Failure)),
            (
                "completed",
                Some(StatusResult::Failure),
                Some(StatusResult::Success),
            ),
            ("error", None, None),
            (
                "unknown",
                Some(StatusResult::Failure),
                Some(StatusResult::Failure),
            ),
        ] {
            let plan = plan_status_actions(
                &check.actions,
                &json!({"message": {"tool": {"result": {"status": status}}}}),
                &EvaluatedAssessment::local(),
                prior,
            )
            .unwrap();
            assert_eq!(
                plan.current_status, expected,
                "{status}, newline {newline:?}"
            );
        }
    }
}
