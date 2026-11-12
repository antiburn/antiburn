use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::Store;

const PROMOTION_COMPLETED_AT_KEY: &str = "internal:promotionOnboardingCompletedAt";
const PROMOTION_DELAY_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PromotionReadiness {
    pub manual: bool,
    pub automatic: bool,
}

const ONBOARDING_STATE_KEY: &str = "internal:desktopOnboardingState";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnboardingState {
    #[default]
    Welcome,
    Sessions,
    Limits,
    Hud,
    Checks,
    Cloud,
    CloudUnavailable,
    Completed,
}

impl Store {
    pub fn onboarding_state(&self) -> Result<OnboardingState> {
        read_onboarding_state(&self.lock())
    }

    /// Clear only onboarding progress for a development replay.
    #[cfg(debug_assertions)]
    pub(crate) fn reset_onboarding(&self) -> Result<()> {
        self.lock().execute(
            "DELETE FROM setting WHERE key IN (?1, ?2)",
            [ONBOARDING_STATE_KEY, PROMOTION_COMPLETED_AT_KEY],
        )?;
        Ok(())
    }

    pub(crate) fn promotion_readiness(
        &self,
        later_session: bool,
        now: i64,
    ) -> Result<PromotionReadiness> {
        let connection = self.lock();
        let manual = read_onboarding_state(&connection)? == OnboardingState::Completed;
        let stored: Option<String> = connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                [PROMOTION_COMPLETED_AT_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let delay_elapsed = match stored {
            None => true,
            Some(value) => value.parse::<i64>().ok().is_some_and(|completed_at| {
                completed_at >= 0 && now.saturating_sub(completed_at) >= PROMOTION_DELAY_SECONDS
            }),
        };
        Ok(PromotionReadiness {
            manual,
            automatic: manual && later_session && delay_elapsed,
        })
    }

    /// Save the current screen. Completion stays final if a draft arrives late.
    pub fn set_onboarding_state(
        &self,
        requested: OnboardingState,
    ) -> Result<(OnboardingState, OnboardingState)> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let previous = read_onboarding_state(&transaction)?;
        if previous == OnboardingState::Completed || previous == requested {
            return Ok((previous, previous));
        }
        transaction.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![ONBOARDING_STATE_KEY, serde_json::to_string(&requested)?],
        )?;
        if requested == OnboardingState::Completed {
            transaction.execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![
                    PROMOTION_COMPLETED_AT_KEY,
                    time::OffsetDateTime::now_utc().unix_timestamp().to_string()
                ],
            )?;
        }
        transaction.commit()?;
        Ok((previous, requested))
    }
}

fn read_onboarding_state(connection: &Connection) -> Result<OnboardingState> {
    let stored = connection
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            [ONBOARDING_STATE_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(stored
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_delay_requires_completion_time_and_later_app_session() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert!(!store.promotion_readiness(true, i64::MAX).unwrap().manual);
        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        let completed_at: String = store
            .lock()
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                [PROMOTION_COMPLETED_AT_KEY],
                |row| row.get(0),
            )
            .unwrap();
        let completed_at = completed_at.parse::<i64>().unwrap();
        assert!(
            store
                .promotion_readiness(false, completed_at)
                .unwrap()
                .manual
        );
        assert!(
            !store
                .promotion_readiness(false, completed_at + PROMOTION_DELAY_SECONDS * 2)
                .unwrap()
                .automatic
        );
        assert!(
            !store
                .promotion_readiness(true, completed_at + PROMOTION_DELAY_SECONDS - 1)
                .unwrap()
                .automatic
        );
        assert!(
            !store
                .promotion_readiness(true, completed_at - 1)
                .unwrap()
                .automatic
        );
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        assert!(
            store
                .promotion_readiness(true, completed_at + PROMOTION_DELAY_SECONDS)
                .unwrap()
                .automatic
        );
        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        assert!(
            store
                .promotion_readiness(true, completed_at + PROMOTION_DELAY_SECONDS)
                .unwrap()
                .automatic
        );
        store
            .set_internal_values_checked(&[(PROMOTION_COMPLETED_AT_KEY, "invalid")])
            .unwrap();
        assert!(!store.promotion_readiness(true, i64::MAX).unwrap().automatic);
        store
            .lock()
            .execute(
                "DELETE FROM setting WHERE key = ?1",
                [PROMOTION_COMPLETED_AT_KEY],
            )
            .unwrap();
        assert!(
            store
                .promotion_readiness(true, completed_at)
                .unwrap()
                .automatic
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn debug_reset_preserves_other_settings_and_allows_replay() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store
            .lock()
            .execute(
                "INSERT INTO setting (key, value) VALUES ('test:preserved', 'saved')",
                [],
            )
            .unwrap();
        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        store.reset_onboarding().unwrap();
        assert_eq!(store.onboarding_state().unwrap(), OnboardingState::Welcome);
        assert!(!store.promotion_readiness(true, i64::MAX).unwrap().automatic);
        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        assert!(
            !store
                .promotion_readiness(true, time::OffsetDateTime::now_utc().unix_timestamp())
                .unwrap()
                .automatic
        );
        store.reset_onboarding().unwrap();
        let preserved: String = store
            .lock()
            .query_row(
                "SELECT value FROM setting WHERE key = 'test:preserved'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, "saved");
        store
            .set_onboarding_state(OnboardingState::Sessions)
            .unwrap();
        assert_eq!(store.onboarding_state().unwrap(), OnboardingState::Sessions);
        store.reset_onboarding().unwrap();
        assert_eq!(store.onboarding_state().unwrap(), OnboardingState::Welcome);
    }

    #[test]
    fn onboarding_draft_survives_reopen_and_settings_writes() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(store.onboarding_state().unwrap(), OnboardingState::Welcome);
        let mut settings = store.settings().unwrap();
        settings.live_usage_enabled = false;
        settings.analytics_enabled = false;
        settings.cloud_upload_enabled = true;
        settings.sidebar_usage_pins = Some(vec![]);
        store.save_settings(&settings).unwrap();
        store
            .set_onboarding_state(OnboardingState::CloudUnavailable)
            .unwrap();
        drop(store);

        let store = Store::open(directory.path()).unwrap();
        assert_eq!(
            store.onboarding_state().unwrap(),
            OnboardingState::CloudUnavailable
        );
        settings.launch_at_login = false;
        store
            .replace_settings_preserving_interface_scale(&settings, |_, _, _| Ok(()))
            .unwrap();
        assert_eq!(
            store
                .set_onboarding_state(OnboardingState::Completed)
                .unwrap(),
            (
                OnboardingState::CloudUnavailable,
                OnboardingState::Completed
            )
        );
        assert_eq!(store.settings().unwrap(), settings);
    }

    #[test]
    fn onboarding_completion_stays_final_after_reopen_and_late_draft_write() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.set_onboarding_state(OnboardingState::Cloud).unwrap();
        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        drop(store);

        let store = Store::open(directory.path()).unwrap();
        assert_eq!(
            store.onboarding_state().unwrap(),
            OnboardingState::Completed
        );
        for requested in [OnboardingState::Checks, OnboardingState::Completed] {
            assert_eq!(
                store.set_onboarding_state(requested).unwrap(),
                (OnboardingState::Completed, OnboardingState::Completed)
            );
        }
        assert!(!store.settings().unwrap().cloud_upload_enabled);
        assert!(store.settings().unwrap().live_usage_active());
    }

    #[test]
    fn onboarding_unknown_draft_falls_back_to_welcome_without_legacy_flag() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store
            .set_internal_values_checked(&[
                ("onboardingCompleted", "true"),
                (ONBOARDING_STATE_KEY, "\"unknown\""),
            ])
            .unwrap();
        assert_eq!(store.onboarding_state().unwrap(), OnboardingState::Welcome);
    }
}
