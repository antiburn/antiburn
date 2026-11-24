//! Closed analytics properties for session visits and controls.

use super::event::Environment;
use antiburn_local::model::AgentKind;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A random UUID v4 that relates events during one visible detail visit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionVisitId([u8; 36]);

impl<'de> Deserialize<'de> for SessionVisitId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let invalid = || serde::de::Error::custom("expected a lowercase UUID v4");
        let bytes: [u8; 36] = value.as_bytes().try_into().map_err(|_| invalid())?;
        let valid_shape = bytes.iter().copied().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                matches!(byte, b'0'..=b'9' | b'a'..=b'f')
            }
        });
        if !valid_shape || bytes[14] != b'4' || !matches!(bytes[19], b'8' | b'9' | b'a' | b'b') {
            return Err(invalid());
        }
        Ok(Self(bytes))
    }
}

impl Serialize for SessionVisitId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = std::str::from_utf8(&self.0).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(value)
    }
}

/// The coarse context collected at a session click, view, or action.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    pub environment: Environment,
    pub activity: SessionActivity,
    pub session_kind: SessionKind,
    pub entry_route: SessionEntryRoute,
    pub recency: SessionRecency,
    pub check_badge: SessionCheckBadge,
}

macro_rules! categories {
    (wire $(#[$doc:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        categories!($(#[$doc])* $name { $($variant => $wire),+ });

        #[cfg(feature = "analytics")]
        impl $name {
            pub(super) const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }
    };
    ($(#[$doc:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Deserialize, Serialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }
    };
}

categories!(/// Activity at the time of capture.
    SessionActivity { Active => "active", Inactive => "inactive", Unknown => "unknown" });
categories!(/// The kind of session selected.
    SessionKind { Session => "session", Subagent => "subagent" });
categories!(/// The navigation that produced a visit.
SessionEntryRoute {
    Popover => "popover", SessionList => "session_list", FilteredList => "filtered_list",
    Adjacent => "adjacent", Related => "related", BurnCheckSample => "burn_check_sample",
    History => "history", Automatic => "automatic", Return => "return",
});
categories!(/// Time since the session's last activity.
SessionRecency {
    UpTo3h => "up_to_3h", Over3hTo24h => "over_3h_to_24h",
    Over24hTo7d => "over_24h_to_7d", Over7d => "over_7d", Unknown => "unknown",
});
categories!(/// Verdict category shown in the available check badge.
SessionCheckBadge {
    Passing => "passing", Failing => "failing", Mixed => "mixed", Unknown => "unknown",
});
categories!(wire /// A session detail pill.
    SessionDetailTab { Context => "context", Cost => "cost", Tools => "tools" });
categories!(wire /// An explicit session detail action.
SessionDetailAction {
    ForkMenuOpened => "fork_menu_opened", OpenParent => "open_parent", OpenChild => "open_child",
    OpenSubagent => "open_subagent", OpenOrchestrator => "open_orchestrator",
    RevealTranscript => "reveal_transcript", CopyPath => "copy_path",
    CopyDiscussionPrompt => "copy_discussion_prompt", RequestRemoval => "request_removal",
    OpenProjectFolder => "open_project_folder", CopyProjectPath => "copy_project_path",
    OpenQuota => "open_quota", OpenMemory => "open_memory", PinCostSeries => "pin_cost_series",
    LimitsExpanded => "limits_expanded",
});
categories!(wire /// A Sessions search or filter activation.
SessionFilterControl {
    SearchFieldClicked => "search_field_clicked", SearchDismissed => "search_dismissed",
    SearchDismissedNoResults => "search_dismissed_no_results",
    SearchDismissedResultsUnavailable => "search_dismissed_results_unavailable",
    SearchDismissedWithResults => "search_dismissed_with_results",
    SearchRetryChecks => "search_retry_checks",
    SearchSessionsAttempted => "search_sessions_attempted",
    SearchAgentAttempted => "search_agent_attempted",
    SearchSettingsAttempted => "search_settings_attempted",
    SearchSessionsOpened => "search_sessions_opened", SearchAgentOpened => "search_agent_opened",
    SearchSettingsOpened => "search_settings_opened",
    FilterMenuOpened => "filter_menu_opened", FilterMenuDismissed => "filter_menu_dismissed",
    AllAgents => "all_agents", AgentOption => "agent_option",
    ResultAll => "result_all", ResultPassing => "result_passing", ResultFailing => "result_failing",
    SpendAll => "spend_all", SpendNotable => "spend_notable", SpendMaterial => "spend_material",
    RemoveAgentChip => "remove_agent_chip", RemoveResultChip => "remove_result_chip",
    RemoveSpendChip => "remove_spend_chip", ClearFilters => "clear_filters",
    ChangeTimeRange => "change_time_range", FindOlderMatches => "find_older_matches",
    RetryList => "retry_list",
});
categories!(wire /// A visible status of an empty filtered list.
    SessionFilterResult { ResultsPending => "results_pending", NoMatches => "no_matches" });
categories!(wire /// The explicit session-list display metric.
    SessionMetric { Cost => "cost", WeeklyPercent => "weekly_percent", FiveHourPercent => "five_hour_percent" });
categories!(wire /// The surface containing the metric control.
    SessionMetricSurface { Popover => "popover", MainWindow => "main_window" });
categories!(wire /// A reminder preset or removal.
    BurnCheckReminderAction { Week => "week", Month => "month", Forever => "forever", Unsnooze => "unsnooze" });
categories!(wire /// Whether saving a reminder action succeeded.
    BurnCheckReminderOutcome { Succeeded => "succeeded", Failed => "failed" });
