use antiburn_checks::{compile, diagnostics::DiagnosticCode};

fn definition(categories: &str, condition: &str) -> String {
    include_str!("../../../docs/burn-check/examples/failed-retry-loop.yml")
        .replace(
            "  tool_result:",
            &format!("  tool_result:\n    tools: [{categories}]"),
        )
        .replace("decisions:", &format!("{condition}decisions:"))
}

#[test]
fn accepts_only_reachable_categories_in_the_supported_subset() {
    assert!(compile(&definition("shell, read, search, edit, other", "")).is_ok());
    for category in ["write", "delegate", "skill", "mcp"] {
        for condition in ["", "if: false\n"] {
            let errors = compile(&definition(category, condition)).unwrap_err();
            assert_eq!(errors.len(), 1, "{category}: {errors:?}");
            assert_eq!(errors[0].code, DiagnosticCode::CompileUnsupportedFeature);
            assert_eq!(
                errors[0].config_path.as_deref(),
                Some("on.tool_result.tools")
            );
            assert!(errors[0].span.is_some());
            assert!(errors[0].message.contains(category));
            assert!(compile(&definition(&format!("shell, {category}"), condition)).is_err());
        }
    }
}

#[test]
fn unknown_category_is_invalid_instead_of_an_unsupported_draft_capability() {
    let errors = compile(&definition("unknown", "")).unwrap_err();
    assert_eq!(errors[0].code, DiagnosticCode::ConfigInvalidValue);
    assert_eq!(errors[0].message, "Unknown normalized tool category.");
}
