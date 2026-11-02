use antiburn_checks::{compile, diagnostics::DiagnosticCode};

fn definition(categories: &str, condition: &str) -> String {
    include_str!("../../../.antiburn/checks/scope-creep.yml")
        .replace(
            "  tool_result:",
            &format!("  tool_result:\n    tools: [{categories}]"),
        )
        .replace("decisions:", &format!("{condition}decisions:"))
}

#[test]
fn accepts_all_check_categories_without_claiming_source_parity() {
    assert!(
        compile(&definition(
            "shell, read, search, edit, write, delegate, skill, mcp, other",
            ""
        ))
        .is_ok()
    );
    for category in [
        "shell", "read", "search", "edit", "write", "delegate", "skill", "mcp", "other",
    ] {
        for condition in ["", "if: false\n"] {
            let compiled = compile(&definition(category, condition)).unwrap();
            assert_eq!(compiled.trigger.tools.unwrap()[0].as_str(), category);
        }
    }
}

#[test]
fn unknown_category_is_invalid_instead_of_an_unsupported_draft_capability() {
    let errors = compile(&definition("unknown", "")).unwrap_err();
    assert_eq!(errors[0].code, DiagnosticCode::ConfigInvalidValue);
    assert_eq!(errors[0].message, "Unknown normalized tool category.");
}
