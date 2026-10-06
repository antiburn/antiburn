//! Persist preferences and publish the last committed settings snapshot.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, params};

use super::{
    AppSettings, DisabledAgents, DiskSpaceDisplay, HiddenMeters, Milestones, NudgePlacement,
    SessionBadgeMetric, Store, ThemePreference, WorkingWeek,
};

impl Store {
    /// Every preference, with defaults filled in for keys never written.
    pub fn settings(&self) -> Result<AppSettings> {
        let connection = self.lock();
        read_settings(&connection)
    }

    /// Return the last committed preferences without waiting for the database.
    pub fn settings_snapshot(&self) -> AppSettings {
        self.settings_snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn update_settings_snapshot(&self, settings: &AppSettings) {
        *self
            .settings_snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings.clone();
    }

    /// Replace every preference, returning what was there and what was stored.
    ///
    /// Reading and writing share one transaction so callers can decide which
    /// shell side effects a transition owes without another writer changing the
    /// answer between the two operations.
    pub fn replace_settings(&self, settings: &AppSettings) -> Result<(AppSettings, AppSettings)> {
        self.replace_settings_with(settings, |_, _| Ok(()))
            .map(|(previous, saved, ())| (previous, saved))
    }

    /// Replace preferences and apply another database change in one transaction.
    pub fn replace_settings_with<T>(
        &self,
        settings: &AppSettings,
        apply: impl FnOnce(&rusqlite::Transaction<'_>, &AppSettings) -> Result<T>,
    ) -> Result<(AppSettings, AppSettings, T)> {
        self.replace_settings_with_transition(settings, |tx, _previous, saved| apply(tx, saved))
    }

    /// Replace preferences and expose both sides of the transition in one transaction.
    pub fn replace_settings_with_transition<T>(
        &self,
        settings: &AppSettings,
        apply: impl FnOnce(&rusqlite::Transaction<'_>, &AppSettings, &AppSettings) -> Result<T>,
    ) -> Result<(AppSettings, AppSettings, T)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let previous = read_settings(&tx)?;
        let saved = settings.clone().normalized();
        write_settings(&tx, &saved)?;
        let result = apply(&tx, &previous, &saved)?;
        tx.commit()?;
        self.update_settings_snapshot(&saved);
        Ok((previous, saved, result))
    }

    /// Replace ordinary preferences without accepting a stale interface scale snapshot.
    pub fn replace_settings_preserving_interface_scale<T>(
        &self,
        settings: &AppSettings,
        apply: impl FnOnce(&rusqlite::Transaction<'_>, &AppSettings, &AppSettings) -> Result<T>,
    ) -> Result<(AppSettings, AppSettings, T)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let previous = read_settings(&tx)?;
        let mut saved = settings.clone();
        saved.interface_scale_percent = previous.interface_scale_percent;
        let saved = saved.normalized();
        write_settings(&tx, &saved)?;
        let result = apply(&tx, &previous, &saved)?;
        tx.commit()?;
        self.update_settings_snapshot(&saved);
        Ok((previous, saved, result))
    }

    /// Change preferences against the latest stored value in one transaction.
    pub fn update_settings(
        &self,
        update: impl FnOnce(&mut AppSettings),
    ) -> Result<(AppSettings, AppSettings)> {
        self.update_settings_with(|settings| {
            update(settings);
            Ok(())
        })
        .map(|(previous, saved, ())| (previous, saved))
    }

    /// Change preferences and validate the request inside the same transaction.
    pub fn update_settings_with<T>(
        &self,
        update: impl FnOnce(&mut AppSettings) -> Result<T>,
    ) -> Result<(AppSettings, AppSettings, T)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let previous = read_settings(&tx)?;
        let mut saved = previous.clone();
        let result = update(&mut saved)?;
        let saved = saved.normalized();
        write_settings(&tx, &saved)?;
        tx.commit()?;
        self.update_settings_snapshot(&saved);
        Ok((previous, saved, result))
    }

    /// Make setup pending without changing the reader's data or choices.
    pub fn restart_onboarding(&self) -> Result<(AppSettings, AppSettings)> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let previous = read_settings(&tx)?;
        let mut saved = previous.clone();
        saved.onboarding_completed = false;
        let saved = saved.normalized();
        write_settings(&tx, &saved)?;
        tx.execute(
            "INSERT INTO setting (key, value) VALUES ('internal:onboardingFlow', 'restart')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )?;
        tx.commit()?;
        self.update_settings_snapshot(&saved);
        Ok((previous, saved))
    }

    /// Whether the pending setup flow came from the explicit restart action.
    pub fn onboarding_flow_is_restart(&self) -> bool {
        self.internal_value("internal:onboardingFlow").as_deref() == Some("restart")
    }

    /// Replace every preference, returning what was actually stored (clamped).
    #[cfg(test)]
    pub fn save_settings(&self, settings: &AppSettings) -> Result<AppSettings> {
        self.replace_settings(settings).map(|(_, saved)| saved)
    }
}

pub(super) fn read_settings(connection: &Connection) -> Result<AppSettings> {
    let mut statement = connection.prepare("SELECT key, value FROM setting")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut stored: HashMap<String, String> = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        stored.insert(key, value);
    }

    let defaults = AppSettings::default();
    Ok(AppSettings {
        theme: stored
            .get("theme")
            .and_then(|value| ThemePreference::parse(value))
            .unwrap_or(defaults.theme),
        interface_scale_percent: stored
            .get("interfaceScalePercent")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.interface_scale_percent),
        activity_window_days: stored
            .get("activityWindowDays")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.activity_window_days),
        session_data_retention_days: stored
            .get("sessionDataRetentionDays")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.session_data_retention_days),
        onboarding_completed: stored
            .get("onboardingCompleted")
            .map(|value| value == "true")
            .unwrap_or(defaults.onboarding_completed),
        launch_at_login: stored
            .get("launchAtLogin")
            .map(|value| value == "true")
            .unwrap_or(defaults.launch_at_login),
        tray_icon_visible: stored
            .get("trayIconVisible")
            .map(|value| value == "true")
            .unwrap_or(defaults.tray_icon_visible),
        dock_icon_visible: stored
            .get("dockIconVisible")
            .map(|value| value == "true")
            .unwrap_or(defaults.dock_icon_visible),
        auto_update: stored
            .get("autoUpdate")
            .map(|value| value == "true")
            .unwrap_or(defaults.auto_update),
        discovery_paused: stored
            .get("discoveryPaused")
            .map(|value| value == "true")
            .unwrap_or(defaults.discovery_paused),
        include_non_repo_folders: stored
            .get("includeNonRepoFolders")
            .map(|value| value == "true")
            .unwrap_or(defaults.include_non_repo_folders),
        notifications_enabled: stored
            .get("notificationsEnabled")
            .map(|value| value == "true")
            .unwrap_or(defaults.notifications_enabled),
        notify_update_available: stored
            .get("notifyUpdateAvailable")
            .map(|value| value == "true")
            .unwrap_or(defaults.notify_update_available),
        notify_scan_failure: stored
            .get("notifyScanFailure")
            .map(|value| value == "true")
            .unwrap_or(defaults.notify_scan_failure),
        nudge_placement: stored
            .get("nudgePlacement")
            .and_then(|value| NudgePlacement::parse(value))
            .unwrap_or(defaults.nudge_placement),
        nudge_auto_dismiss_secs: stored
            .get("nudgeAutoDismissSecs")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.nudge_auto_dismiss_secs),
        notification_sound: stored
            .get("notificationSound")
            .map(|value| value == "true")
            .unwrap_or(defaults.notification_sound),
        nudges_respect_dnd: stored
            .get("nudgesRespectDnd")
            .map(|value| value == "true")
            .unwrap_or(defaults.nudges_respect_dnd),
        disk_space_display: stored
            .get("diskSpaceDisplay")
            .and_then(|value| DiskSpaceDisplay::parse(value))
            .unwrap_or(defaults.disk_space_display),
        disk_space_threshold_gb: stored
            .get("diskSpaceThresholdGb")
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.disk_space_threshold_gb),
        notify_disk_space_low: stored
            .get("notifyDiskSpaceLow")
            .map(|value| value == "true")
            .unwrap_or(defaults.notify_disk_space_low),
        milestones_5h: stored
            .get("milestonePercentages5h")
            .map(|value| Milestones::parse(value))
            .unwrap_or(defaults.milestones_5h),
        milestones_weekly: stored
            .get("milestonePercentagesWeekly")
            .map(|value| Milestones::parse(value))
            .unwrap_or(defaults.milestones_weekly),
        live_usage_enabled: stored
            .get("liveUsageEnabled")
            .map(|value| value == "true")
            .unwrap_or(defaults.live_usage_enabled),
        live_usage_hidden_providers: stored
            .get("liveUsageHiddenProviders")
            .map(|value| HiddenMeters::parse(value))
            .unwrap_or(defaults.live_usage_hidden_providers.clone()),
        disabled_agents: stored
            .get("disabledAgents")
            .map(|value| DisabledAgents::parse(value))
            .unwrap_or(defaults.disabled_agents.clone()),
        // No stored answer means this database predates the setting. A fresh
        // install takes the default. An install that already finished setup
        // stays off until the reader enables it.
        analytics_enabled: stored
            .get("analyticsEnabled")
            .map(|value| value == "true")
            .unwrap_or_else(|| {
                let finished = stored
                    .get("onboardingCompleted")
                    .map(|value| value == "true")
                    .unwrap_or(false);
                !finished && defaults.analytics_enabled
            }),
        overview_limits_expanded: stored
            .get("overviewLimitsExpanded")
            .map(|value| value == "true")
            .unwrap_or(defaults.overview_limits_expanded),
        skills_mcp_expanded: stored
            .get("skillsMcpExpanded")
            .map(|value| value == "true")
            .unwrap_or(defaults.skills_mcp_expanded),
        session_badge_metric: stored
            .get("sessionBadgeMetric")
            .and_then(|value| SessionBadgeMetric::parse(value))
            .unwrap_or(defaults.session_badge_metric),
        session_filter: stored
            .get("sessionFilter")
            .cloned()
            .unwrap_or_else(|| defaults.session_filter.clone()),
        // An unreadable or unknown value falls back to the default, which
        // counts every day. A preference this side cannot read is not an
        // instruction to hold a marker still.
        working_week: stored
            .get("workingWeek")
            .and_then(|value| WorkingWeek::parse(value))
            .unwrap_or(defaults.working_week),
    }
    .normalized())
}

/// Write every normalized preference through one transaction.
fn write_settings(connection: &Connection, settings: &AppSettings) -> Result<()> {
    let mut put = connection.prepare(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?;
    put.execute(params!["theme", settings.theme.as_str()])?;
    put.execute(params![
        "interfaceScalePercent",
        settings.interface_scale_percent.to_string()
    ])?;
    put.execute(params![
        "activityWindowDays",
        settings.activity_window_days.to_string()
    ])?;
    put.execute(params![
        "sessionDataRetentionDays",
        settings.session_data_retention_days.to_string()
    ])?;
    put.execute(params![
        "onboardingCompleted",
        bool_text(settings.onboarding_completed)
    ])?;
    put.execute(params![
        "launchAtLogin",
        bool_text(settings.launch_at_login)
    ])?;
    put.execute(params![
        "trayIconVisible",
        bool_text(settings.tray_icon_visible)
    ])?;
    put.execute(params![
        "dockIconVisible",
        bool_text(settings.dock_icon_visible)
    ])?;
    put.execute(params!["autoUpdate", bool_text(settings.auto_update)])?;
    put.execute(params![
        "discoveryPaused",
        bool_text(settings.discovery_paused)
    ])?;
    put.execute(params![
        "includeNonRepoFolders",
        bool_text(settings.include_non_repo_folders)
    ])?;
    put.execute(params![
        "notificationsEnabled",
        bool_text(settings.notifications_enabled)
    ])?;
    put.execute(params![
        "notifyUpdateAvailable",
        bool_text(settings.notify_update_available)
    ])?;
    put.execute(params![
        "notifyScanFailure",
        bool_text(settings.notify_scan_failure)
    ])?;
    put.execute(params!["nudgePlacement", settings.nudge_placement.as_str()])?;
    put.execute(params![
        "nudgeAutoDismissSecs",
        settings.nudge_auto_dismiss_secs.to_string()
    ])?;
    put.execute(params![
        "notificationSound",
        bool_text(settings.notification_sound)
    ])?;
    put.execute(params![
        "nudgesRespectDnd",
        bool_text(settings.nudges_respect_dnd)
    ])?;
    put.execute(params![
        "diskSpaceDisplay",
        settings.disk_space_display.as_str()
    ])?;
    put.execute(params![
        "diskSpaceThresholdGb",
        settings.disk_space_threshold_gb.to_string()
    ])?;
    put.execute(params![
        "notifyDiskSpaceLow",
        bool_text(settings.notify_disk_space_low)
    ])?;
    put.execute(params![
        "milestonePercentages5h",
        settings.milestones_5h.as_str()
    ])?;
    put.execute(params![
        "milestonePercentagesWeekly",
        settings.milestones_weekly.as_str()
    ])?;
    put.execute(params![
        "liveUsageEnabled",
        bool_text(settings.live_usage_enabled)
    ])?;
    put.execute(params![
        "liveUsageHiddenProviders",
        settings.live_usage_hidden_providers.as_str()
    ])?;
    put.execute(params!["disabledAgents", settings.disabled_agents.as_str()])?;
    put.execute(params![
        "analyticsEnabled",
        bool_text(settings.analytics_enabled)
    ])?;
    put.execute(params![
        "overviewLimitsExpanded",
        bool_text(settings.overview_limits_expanded)
    ])?;
    put.execute(params![
        "skillsMcpExpanded",
        bool_text(settings.skills_mcp_expanded)
    ])?;
    put.execute(params![
        "sessionBadgeMetric",
        settings.session_badge_metric.as_str()
    ])?;
    put.execute(params!["sessionFilter", settings.session_filter.as_str()])?;
    put.execute(params!["workingWeek", settings.working_week.as_str()])?;
    Ok(())
}

/// `true`/`false` as the text the setting table stores.
fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}
