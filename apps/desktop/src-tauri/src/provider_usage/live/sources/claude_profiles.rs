//! Claude usage for the default directory and every added Claude profile.
//!
//! The default [`ClaudeDirectFetch`] answers for the CLI's default directory,
//! the same as before profiles existed: its detection, its errors, and its
//! analytics diagnostic are this source's own. Each profile the reader adds
//! in Settings gets its own [`ClaudeDirectFetch::for_profile`], with its own
//! cooldown and cache, and adds its snapshot labelled with the reader's name
//! for it.
//!
//! Each failure names the profile it belongs to, so the views attach it only
//! to that profile's reading and a failure in one profile never marks
//! another account as failed. A profile with no subscription login, such as
//! a directory that a wrapper script points at an API-key provider, reports
//! nothing and so shows no meter.
//!
//! # Cost
//!
//! The profiles run one after another inside the one pass. Each profile's
//! cooldown limits it to one usage request per pass, and only when its
//! cooldown permits. In the worst case one pass makes 16 sequential
//! requests (`MAX_PROFILES`), each bounded by the 15-second HTTP timeout. On
//! macOS each request first reads its Keychain item's attributes, and the
//! secret only when the item changed or the cached token expired: one or two
//! `security` subprocesses, each with a 3-second deadline. A CLI refresh runs
//! in user context only.
//! A rate-limited profile sends nothing until its `Retry-After` passes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::claude_profiles;
use crate::provider_usage::live::model::Presence;
use crate::provider_usage::live::{AccountFailure, LiveUsageSource, SourceOutcome};

use super::anthropic_fetch::ClaudeDirectFetch;

struct ProfileFetch {
    config_dir: PathBuf,
    fetch: ClaudeDirectFetch,
}

pub struct ClaudeProfilesFetch {
    default: ClaudeDirectFetch,
    /// One fetch per added profile, kept across passes so each keeps its
    /// cooldown and last good reading.
    profiles: Mutex<Vec<Arc<ProfileFetch>>>,
}

impl ClaudeProfilesFetch {
    pub fn new() -> ClaudeProfilesFetch {
        ClaudeProfilesFetch {
            default: ClaudeDirectFetch::new(),
            profiles: Mutex::new(Vec::new()),
        }
    }

    /// The fetch and label for each registered profile, in registration
    /// order. This drops the fetch of a removed profile and makes a new
    /// fetch for a new profile.
    fn current_profiles(&self) -> Vec<(Arc<ProfileFetch>, String)> {
        self.profiles_for(claude_profiles::registered_profiles())
    }

    fn profiles_for(
        &self,
        registered: Vec<claude_profiles::RegisteredProfile>,
    ) -> Vec<(Arc<ProfileFetch>, String)> {
        let mut cached = self
            .profiles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cached.retain(|profile| {
            registered
                .iter()
                .any(|entry| entry.config_dir == profile.config_dir)
        });
        registered
            .into_iter()
            .map(|entry| {
                let fetch = match cached
                    .iter()
                    .find(|profile| profile.config_dir == entry.config_dir)
                {
                    Some(fetch) => Arc::clone(fetch),
                    None => {
                        let fetch = Arc::new(ProfileFetch {
                            fetch: ClaudeDirectFetch::for_profile(entry.config_dir.clone()),
                            config_dir: entry.config_dir,
                        });
                        cached.push(Arc::clone(&fetch));
                        fetch
                    }
                };
                (fetch, entry.label)
            })
            .collect()
    }
}

impl Default for ClaudeProfilesFetch {
    fn default() -> ClaudeProfilesFetch {
        ClaudeProfilesFetch::new()
    }
}

impl LiveUsageSource for ClaudeProfilesFetch {
    fn id(&self) -> &'static str {
        self.default.id()
    }

    fn provider(&self) -> &'static str {
        self.default.provider()
    }

    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn detect(&self, online: bool) -> Presence {
        self.default.detect(online)
    }

    fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
        let mut outcome = self.default.fetch(max_age);
        let default_label = claude_profiles::default_reading_label();
        for snapshot in &mut outcome.snapshots {
            snapshot.account_label = default_label.clone();
        }
        outcome.account_label = default_label;
        // Read the list before the requests, so a Settings change never
        // waits for a profile's network call.
        for (profile, label) in self.current_profiles() {
            let profile_outcome = profile.fetch.fetch(max_age);
            if let Some(error) = profile_outcome.error {
                outcome.account_failures.push(AccountFailure {
                    account_label: label.clone(),
                    error,
                    detail: profile_outcome.detail,
                    retry_at: profile_outcome.retry_at,
                });
            }
            outcome
                .snapshots
                .extend(profile_outcome.snapshots.into_iter().map(|mut snapshot| {
                    snapshot.account_label = Some(label.clone());
                    snapshot
                }));
        }
        outcome
    }

    #[cfg(feature = "analytics")]
    fn analytics_diagnostic(&self) -> Option<crate::provider_usage::live::AnalyticsDiagnostic> {
        self.default.analytics_diagnostic()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_fetches_follow_the_registry_and_survive_between_passes() {
        let source = ClaudeProfilesFetch::new();
        let profile = |dir: &str, label: &str| claude_profiles::RegisteredProfile {
            config_dir: std::path::PathBuf::from(dir),
            label: label.into(),
        };
        let first = source.profiles_for(vec![
            profile("/work/.claude-a", "A"),
            profile("/work/.claude-b", "B"),
        ]);
        let second = source.profiles_for(vec![profile("/work/.claude-b", "Renamed")]);
        let third = source.profiles_for(Vec::new());

        let labels: Vec<&str> = first.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(labels, ["A", "B"]);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].1, "Renamed");
        assert!(Arc::ptr_eq(&first[1].0, &second[0].0));
        assert!(third.is_empty());
        assert!(source.profiles.lock().unwrap().is_empty());
    }
}
