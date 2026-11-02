//! Share consumer discovery without sharing location failures.

use crate::opencode_live::config::{AuthorityError, VerifiedConfigConsumer, canonical_location};
pub(crate) use crate::opencode_live::config::{ConfigEntry, CurrentConfigAuthority};

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_CAPTURE_DURATION: Duration = Duration::from_secs(3);
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const MAX_REPORT_LOCATIONS: usize = 256;
const MAX_WORKER_LOCATIONS: usize = 8;
const FAILURE_CACHE_AGE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureUnavailable {
    Discovery,
    Location,
    Budget,
}

#[derive(Clone, Copy)]
enum CachePolicy {
    Report,
    Worker,
}

struct CachedCapture {
    receipt: Result<CurrentConfigAuthority, CaptureUnavailable>,
    observed_at: Instant,
    touched: u64,
    bytes: usize,
}

/// Reports share a short pass budget. Workers evict locations and refresh receipts.
pub(crate) struct CurrentConfigAuthorities {
    captures: BTreeMap<PathBuf, CachedCapture>,
    consumer: Option<VerifiedConfigConsumer>,
    discovery_failed: bool,
    started: Option<Instant>,
    retained_bytes: usize,
    sequence: u64,
    policy: CachePolicy,
}

impl Default for CurrentConfigAuthorities {
    fn default() -> Self {
        Self {
            captures: BTreeMap::new(),
            consumer: None,
            discovery_failed: false,
            started: None,
            retained_bytes: 0,
            sequence: 0,
            policy: CachePolicy::Report,
        }
    }
}

impl CurrentConfigAuthorities {
    pub(crate) fn for_worker() -> Self {
        Self {
            policy: CachePolicy::Worker,
            ..Self::default()
        }
    }

    /// Blocks the calling thread. Call this outside the store snapshot transaction.
    pub(crate) fn capture(&mut self, location: &Path) -> Option<&CurrentConfigAuthority> {
        self.try_capture(location).ok()
    }

    pub(crate) fn try_capture(
        &mut self,
        location: &Path,
    ) -> Result<&CurrentConfigAuthority, CaptureUnavailable> {
        tauri::async_runtime::block_on(self.try_capture_async(location))
    }

    pub(crate) async fn capture_async(
        &mut self,
        location: &Path,
    ) -> Option<&CurrentConfigAuthority> {
        self.try_capture_async(location).await.ok()
    }

    async fn try_capture_async(
        &mut self,
        location: &Path,
    ) -> Result<&CurrentConfigAuthority, CaptureUnavailable> {
        self.capture_with(
            location,
            VerifiedConfigConsumer::registered_async,
            |mut consumer, location, remaining| async move {
                let receipt = tokio::time::timeout(remaining, consumer.capture_async(&location))
                    .await
                    .map_err(|_| CaptureUnavailable::Budget)
                    .and_then(|result| result.map_err(|_| CaptureUnavailable::Location));
                (consumer, receipt)
            },
        )
        .await
    }

    async fn capture_with<D, DF, C, CF>(
        &mut self,
        location: &Path,
        discover: D,
        capture: C,
    ) -> Result<&CurrentConfigAuthority, CaptureUnavailable>
    where
        D: FnOnce() -> DF,
        DF: Future<Output = Result<VerifiedConfigConsumer, AuthorityError>>,
        C: FnOnce(VerifiedConfigConsumer, PathBuf, Duration) -> CF,
        CF: Future<
            Output = (
                VerifiedConfigConsumer,
                Result<CurrentConfigAuthority, CaptureUnavailable>,
            ),
        >,
    {
        let lookup_started = Instant::now();
        let key = location.to_path_buf();
        let location =
            tokio::time::timeout(MAX_CAPTURE_DURATION, canonical_location(location.into()))
                .await
                .map_err(|_| CaptureUnavailable::Budget)?
                .map_err(|_| CaptureUnavailable::Location)?;
        if self.cached_fresh(&key, &location) {
            self.sequence = self.sequence.saturating_add(1);
            self.captures
                .get_mut(&key)
                .expect("cached location")
                .touched = self.sequence;
            return self.cached_result(&key);
        }
        self.remove(&key);
        if matches!(self.policy, CachePolicy::Report) && self.captures.len() >= MAX_REPORT_LOCATIONS
        {
            return Err(CaptureUnavailable::Budget);
        }
        if self.discovery_failed {
            return Err(CaptureUnavailable::Discovery);
        }
        let remaining = self.remaining(lookup_started)?;
        if self.consumer.is_none() {
            match tokio::time::timeout(remaining, discover()).await {
                Err(_) => return Err(CaptureUnavailable::Budget),
                Ok(Err(_)) => {
                    self.discovery_failed = true;
                    return Err(CaptureUnavailable::Discovery);
                }
                Ok(Ok(consumer)) => self.consumer = Some(consumer),
            }
        }
        let remaining = self.remaining(lookup_started)?;
        let consumer = self.consumer.take().expect("consumer discovery succeeded");
        let (consumer, receipt) = capture(consumer, location.clone(), remaining).await;
        self.consumer = Some(consumer);
        self.retain(key.clone(), receipt)?;
        self.cached_result(&key)
    }

    fn remaining(&mut self, lookup_started: Instant) -> Result<Duration, CaptureUnavailable> {
        let started = match self.policy {
            CachePolicy::Report => *self.started.get_or_insert(lookup_started),
            CachePolicy::Worker => lookup_started,
        };
        MAX_CAPTURE_DURATION
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(CaptureUnavailable::Budget)
    }

    fn cached_fresh(&self, location: &Path, canonical: &Path) -> bool {
        self.captures
            .get(location)
            .is_some_and(|cached| match &cached.receipt {
                Ok(receipt) => !receipt.is_expired() && receipt.location() == canonical,
                Err(_) => {
                    matches!(self.policy, CachePolicy::Report)
                        || cached.observed_at.elapsed() < FAILURE_CACHE_AGE
                }
            })
    }

    fn cached_result(
        &self,
        location: &Path,
    ) -> Result<&CurrentConfigAuthority, CaptureUnavailable> {
        self.captures
            .get(location)
            .ok_or(CaptureUnavailable::Budget)?
            .receipt
            .as_ref()
            .map_err(|reason| *reason)
    }

    fn retain(
        &mut self,
        location: PathBuf,
        receipt: Result<CurrentConfigAuthority, CaptureUnavailable>,
    ) -> Result<(), CaptureUnavailable> {
        let key_bytes = location
            .as_os_str()
            .len()
            .saturating_mul(2)
            .saturating_add(std::mem::size_of::<CachedCapture>())
            .saturating_add(std::mem::size_of::<PathBuf>() + 4 * std::mem::size_of::<usize>());
        let seed_bytes = self
            .consumer
            .as_ref()
            .map_or(0, VerifiedConfigConsumer::retained_bytes);
        if seed_bytes.saturating_add(key_bytes) > MAX_CAPTURE_BYTES {
            return Err(CaptureUnavailable::Budget);
        }
        let receipt_bytes = receipt
            .as_ref()
            .map_or(0, CurrentConfigAuthority::retained_bytes);
        let (receipt, bytes) = if seed_bytes
            .saturating_add(key_bytes)
            .saturating_add(receipt_bytes)
            > MAX_CAPTURE_BYTES
        {
            (Err(CaptureUnavailable::Budget), key_bytes)
        } else {
            (receipt, key_bytes.saturating_add(receipt_bytes))
        };
        let limit = match self.policy {
            CachePolicy::Report => MAX_REPORT_LOCATIONS,
            CachePolicy::Worker => MAX_WORKER_LOCATIONS,
        };
        while self.captures.len() >= limit
            || self
                .retained_bytes
                .saturating_add(seed_bytes)
                .saturating_add(bytes)
                > MAX_CAPTURE_BYTES
        {
            if matches!(self.policy, CachePolicy::Report) {
                return Err(CaptureUnavailable::Budget);
            }
            let oldest = self
                .captures
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(path, _)| path.clone())
                .ok_or(CaptureUnavailable::Budget)?;
            self.remove(&oldest);
        }
        self.sequence = self.sequence.saturating_add(1);
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
        self.captures.insert(
            location,
            CachedCapture {
                receipt,
                observed_at: Instant::now(),
                touched: self.sequence,
                bytes,
            },
        );
        Ok(())
    }

    fn remove(&mut self, location: &Path) {
        if let Some(old) = self.captures.remove(location) {
            self.retained_bytes = self.retained_bytes.saturating_sub(old.bytes);
        }
    }

    pub(crate) fn get(&self, location: &Path) -> Option<&CurrentConfigAuthority> {
        self.cached_result(location)
            .ok()
            .filter(|receipt| !receipt.is_expired())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture_capture(cache: &mut CurrentConfigAuthorities, location: &Path) -> bool {
        let seed_path = location.to_path_buf();
        cache
            .capture_with(
                location,
                || async move { Ok(VerifiedConfigConsumer::fixture(&seed_path)) },
                |consumer, path, _| async move {
                    (
                        consumer,
                        Ok(CurrentConfigAuthority::fixture(
                            &path,
                            serde_json::json!([]),
                        )),
                    )
                },
            )
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn invalid_first_location_does_not_attempt_or_disable_discovery() {
        for mut cache in [
            CurrentConfigAuthorities::default(),
            CurrentConfigAuthorities::for_worker(),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            assert_eq!(
                cache
                    .capture_with(
                        &temporary.path().join("deleted"),
                        || async { panic!("invalid cwd must not discover") },
                        |_, _, _| async { panic!("invalid cwd must not capture") }
                    )
                    .await
                    .err(),
                Some(CaptureUnavailable::Location)
            );
            assert!(cache.started.is_none());
            assert!(fixture_capture(&mut cache, temporary.path()).await);
        }
    }

    #[tokio::test]
    async fn first_config_failure_keeps_the_discovered_consumer() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join("first");
        std::fs::create_dir(&first).unwrap();
        let mut cache = CurrentConfigAuthorities::default();
        assert_eq!(
            cache
                .capture_with(
                    &first,
                    || async { Ok(VerifiedConfigConsumer::fixture(temporary.path())) },
                    |consumer, _, _| async { (consumer, Err(CaptureUnavailable::Location)) }
                )
                .await
                .err(),
            Some(CaptureUnavailable::Location)
        );
        assert!(cache.consumer.is_some());
        assert!(
            cache
                .capture_with(
                    temporary.path(),
                    || async { panic!("consumer must survive a location failure") },
                    |consumer, path, _| async move {
                        (
                            consumer,
                            Ok(CurrentConfigAuthority::fixture(
                                &path,
                                serde_json::json!([]),
                            )),
                        )
                    }
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn failed_service_discovery_is_cached_once() {
        let temporary = tempfile::tempdir().unwrap();
        let mut cache = CurrentConfigAuthorities::default();
        assert_eq!(
            cache
                .capture_with(
                    temporary.path(),
                    || async { Err(AuthorityError::Unavailable) },
                    |_, _, _| async { panic!("unavailable consumer must not capture") }
                )
                .await
                .err(),
            Some(CaptureUnavailable::Discovery)
        );
        assert_eq!(
            cache
                .capture_with(
                    temporary.path(),
                    || async { panic!("discovery must not repeat") },
                    |_, _, _| async { panic!("unavailable consumer must not capture") }
                )
                .await
                .err(),
            Some(CaptureUnavailable::Discovery)
        );
    }

    #[tokio::test]
    async fn worker_first_eligible_lookup_starts_lazily_and_ignores_backlog_age() {
        let temporary = tempfile::tempdir().unwrap();
        let mut cache = CurrentConfigAuthorities::for_worker();
        assert!(cache.started.is_none());
        tokio::time::sleep(MAX_CAPTURE_DURATION + Duration::from_millis(50)).await;
        assert!(fixture_capture(&mut cache, temporary.path()).await);
    }

    #[tokio::test]
    async fn expired_worker_receipt_is_refreshed_from_the_seed() {
        let temporary = tempfile::tempdir().unwrap();
        let location = temporary.path().canonicalize().unwrap();
        let mut cache = CurrentConfigAuthorities::for_worker();
        assert!(fixture_capture(&mut cache, &location).await);
        cache.captures.get_mut(&location).unwrap().receipt =
            Ok(CurrentConfigAuthority::fixture(&location, serde_json::json!([])).expired_fixture());
        assert!(
            cache
                .capture_with(
                    &location,
                    || async { panic!("refresh must not discover") },
                    |consumer, path, _| async move {
                        (
                            consumer,
                            Ok(CurrentConfigAuthority::fixture(
                                &path,
                                serde_json::json!([]),
                            )),
                        )
                    }
                )
                .await
                .unwrap()
                .entries()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn worker_evicts_locations_instead_of_rejecting_the_ninth_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let mut cache = CurrentConfigAuthorities::for_worker();
        for index in 0..12 {
            let location = temporary.path().join(index.to_string());
            std::fs::create_dir(&location).unwrap();
            assert!(fixture_capture(&mut cache, &location).await);
            assert!(cache.captures.len() <= MAX_WORKER_LOCATIONS);
            assert!(cache.retained_bytes < MAX_CAPTURE_BYTES);
        }
        assert!(!cache.captures.contains_key(&temporary.path().join("0")));
        assert!(cache.consumer.is_some());
    }

    #[tokio::test]
    async fn a_receipt_budget_failure_keeps_discovery_available_for_other_locations() {
        let temporary = tempfile::tempdir().unwrap();
        let next = temporary.path().join("next");
        std::fs::create_dir(&next).unwrap();
        for mut cache in [
            CurrentConfigAuthorities::default(),
            CurrentConfigAuthorities::for_worker(),
        ] {
            assert_eq!(cache.capture_with(temporary.path(), || async { Ok(VerifiedConfigConsumer::fixture(temporary.path())) }, |consumer, path, _| async move {
                (consumer, Ok(CurrentConfigAuthority::fixture(&path, serde_json::json!([
                    {"type":"document","info":{"large":"x".repeat(MAX_CAPTURE_BYTES / 64)}}
                ]))))
            }).await.err(), Some(CaptureUnavailable::Budget));
            assert!(cache.consumer.is_some());
            assert!(!cache.discovery_failed);
            assert!(
                cache
                    .capture_with(
                        &next,
                        || async { panic!("budget exhaustion must not rediscover") },
                        |consumer, path, _| async move {
                            (
                                consumer,
                                Ok(CurrentConfigAuthority::fixture(
                                    &path,
                                    serde_json::json!([]),
                                )),
                            )
                        }
                    )
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn report_budget_does_not_discard_the_consumer_seed() {
        let temporary = tempfile::tempdir().unwrap();
        let mut cache = CurrentConfigAuthorities::default();
        assert!(fixture_capture(&mut cache, temporary.path()).await);
        cache.started = Some(Instant::now() - MAX_CAPTURE_DURATION);
        let next = temporary.path().join("next");
        std::fs::create_dir(&next).unwrap();
        assert_eq!(
            cache
                .capture_with(
                    &next,
                    || async { panic!("budget must stop discovery") },
                    |_, _, _| async { panic!("budget must stop capture") }
                )
                .await
                .err(),
            Some(CaptureUnavailable::Budget)
        );
        assert!(cache.consumer.is_some());
        assert!(cache.get(temporary.path()).is_some());
    }
}
