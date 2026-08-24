// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use antiburn_nudge::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_assembles_full_payload() {
        let n = Nudge::new(
            "usage-42",
            NudgeKind::UsageMilestone,
            NudgeTone::Info,
            "75% of your weekly limit used",
            "The provider reported this usage milestone.",
            "The percentage comes from the provider's current usage reading.",
        )
        .recommendations(vec![
            "Review usage".into(),
            "Check provider dashboard".into(),
        ])
        .action("view_usage", "View usage", true)
        .action("dismiss", "Dismiss", false)
        .timeout_ms(8000);

        assert_eq!(n.actions.len(), 2);
        // The primary action always sorts last, regardless of call order.
        assert!(n.actions.last().unwrap().primary);
        assert_eq!(n.recommendations.len(), 2);
        assert_eq!(n.timeout_ms, Some(8000));
    }

    #[test]
    fn primary_action_sorts_last_regardless_of_call_order() {
        let primary_first = Nudge::new(
            "n-1",
            NudgeKind::UsageMilestone,
            NudgeTone::Warning,
            "T",
            "S",
            "D",
        )
        .action("view_usage", "View usage", true)
        .action("dismiss", "Dismiss", false);
        assert_eq!(primary_first.actions[0].id, "dismiss");
        assert_eq!(primary_first.actions[1].id, "view_usage");

        let primary_last = Nudge::new(
            "n-2",
            NudgeKind::UsageMilestone,
            NudgeTone::Warning,
            "T",
            "S",
            "D",
        )
        .action("dismiss", "Dismiss", false)
        .action("view_usage", "View usage", true);

        assert_eq!(
            primary_first.actions, primary_last.actions,
            "call order must not affect the final action order"
        );
    }

    #[test]
    fn serde_round_trips_and_omits_empty_optionals() {
        let minimal = Nudge::new(
            "u-1",
            NudgeKind::UpdateAvailable,
            NudgeTone::Info,
            "A new version is available",
            "Version 0.2.0 is available.",
            "Open the About pane to see the update status.",
        )
        .action("open_updates", "Update", true);

        let json = serde_json::to_string(&minimal).unwrap();
        // The kind uses camelCase. Empty optional fields do not appear.
        assert!(json.contains("\"kind\":\"updateAvailable\""));
        assert!(json.contains("\"subtitle\""));
        assert!(json.contains("\"description\""));
        assert!(!json.contains("recommendations"));
        assert!(!json.contains("timeoutMs"));
        assert!(!json.contains("actor"));

        let back: Nudge = serde_json::from_str(&json).unwrap();
        assert_eq!(back, minimal);
    }

    /// A name that is only whitespace is nobody: the app may key behavior off
    /// this field, and an empty string would read as a person who doesn't exist.
    #[test]
    fn a_blank_actor_is_nobody() {
        let named = Nudge::new(
            "a-1",
            NudgeKind::UsageMilestone,
            NudgeTone::Info,
            "T",
            "S",
            "D",
        )
        .actor(Some("claude"));
        assert_eq!(named.actor.as_deref(), Some("claude"));

        for blank in ["", "   "] {
            let nudge = Nudge::new(
                "a-2",
                NudgeKind::UsageMilestone,
                NudgeTone::Info,
                "T",
                "S",
                "D",
            )
            .actor(Some(blank));
            assert_eq!(nudge.actor, None, "{blank:?} should be nobody");
        }

        let none = Nudge::new(
            "a-3",
            NudgeKind::UsageMilestone,
            NudgeTone::Info,
            "T",
            "S",
            "D",
        )
        .actor(None::<String>);
        assert_eq!(none.actor, None);
    }

    #[test]
    fn production_kinds_serialize_as_exact_nudge_types() {
        assert_eq!(
            serde_json::to_string(&NudgeKind::UpdateAvailable).unwrap(),
            "\"updateAvailable\""
        );
        assert_eq!(
            serde_json::to_string(&NudgeKind::ScanFailure).unwrap(),
            "\"scanFailure\""
        );
        assert_eq!(
            serde_json::to_string(&NudgeKind::DiskSpaceLow).unwrap(),
            "\"diskSpaceLow\""
        );
        assert_eq!(
            serde_json::to_string(&NudgeKind::UsageMilestone).unwrap(),
            "\"usageMilestone\""
        );
        assert_eq!(
            serde_json::to_string(&NudgeKind::MenuBarLocation).unwrap(),
            "\"menuBarLocation\""
        );
        assert_eq!(serde_json::to_string(&NudgeKind::Test).unwrap(), "\"test\"");
    }

    #[test]
    fn action_target_serializes_as_tagged_payload() {
        let n = Nudge::new(
            "session-1",
            NudgeKind::UsageMilestone,
            NudgeTone::Warning,
            "75% of your weekly limit used",
            "The provider reported this usage milestone.",
            "The percentage comes from the provider's current usage reading.",
        )
        .action_with_target(
            "view_session",
            "View session",
            true,
            NudgeActionTarget::Session {
                agent: "claude".into(),
                session_id: "session-1".into(),
                environment: None,
            },
        );

        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains("\"target\""));
        assert!(json.contains("\"type\":\"session\""));
        assert!(json.contains("\"sessionId\":\"session-1\""));
    }

    #[test]
    fn action_event_uses_camel_case_kind() {
        let ev = NudgeActionEvent {
            kind: NudgeKind::UsageMilestone,
            action_id: "view_usage".into(),
            target: Some(NudgeActionTarget::ProviderUsage {
                provider: "openai".into(),
                account_key: Some("account-key".into()),
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"usageMilestone\""));
        assert!(json.contains("\"actionId\":\"view_usage\""));
        assert!(json.contains("\"type\":\"providerUsage\""));
    }
}
