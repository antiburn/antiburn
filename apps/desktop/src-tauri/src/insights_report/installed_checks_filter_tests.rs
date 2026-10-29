use antiburn_checks::check_store::SourceProgress;
use antiburn_checks::filters::{AttributionScope, FilterDiagnostic, FilterField, FilterReason};

use super::*;

#[test]
fn saved_filter_limits_reach_reports_without_attempts_or_raw_source_details() {
    for (reason, code) in [
        (
            FilterReason::Unavailable,
            Some("filter-evidence-unavailable"),
        ),
        (FilterReason::Incomplete, Some("filter-evidence-incomplete")),
        (FilterReason::Invalid, Some("filter-evidence-invalid")),
        (FilterReason::LimitExceeded, Some("filter-work-limit")),
        (FilterReason::KnownEmpty, None),
        (FilterReason::NoMatch, None),
        (FilterReason::IgnoredPath, None),
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        session(&store, "filtered", 100);
        let branch =
            BranchId(SessionKey::new("native", "claude-code", "filtered").to_identity_json());
        let mut progress = SourceProgress::default();
        progress
            .selection
            .branches
            .entry(branch)
            .or_default()
            .last_filter_diagnostic = Some(FilterDiagnostic {
            field: FilterField::PathsIgnore,
            reason,
            scope: AttributionScope::Message,
            limits: vec!["private source detail".into()],
        });
        store
            .lock()
            .execute(
                "UPDATE burn_check_activation SET checkpoint_json = ?1 WHERE check_id = ?2",
                params![progress.encode().unwrap(), definition.check_id],
            )
            .unwrap();

        let report = read_installed_checks_in_window(&store, &[definition], &request()).unwrap();
        let matching = report[0]
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code.starts_with("filter-"));
        assert_eq!(matching.map(|diagnostic| diagnostic.code.as_str()), code);
        if let Some(diagnostic) = matching {
            assert_eq!(diagnostic.severity, InstalledCheckDiagnosticSeverity::Info);
            assert_eq!(diagnostic.affected_session_count, 1);
            assert!(
                diagnostic
                    .details
                    .iter()
                    .any(|detail| detail.label == "Filter" && detail.value == "paths-ignore")
            );
        }
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("private source detail")
        );
        assert_eq!(report[0].finding, 0);
        assert_eq!(report[0].clean, 0);
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
    }
}
