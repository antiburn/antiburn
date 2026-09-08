//! Bounded runtime resource sampling for anonymised analytics.

mod aggregate;
mod platform;
pub mod schema;

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::Manager as _;

use self::aggregate::ResourceUsageAccumulator;
use self::platform::ProcessSample;
use self::schema::ResourceUsageSummary;
use crate::Schedulers;
use crate::store::{Store, database_path};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REPORT_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Default)]
pub(super) struct ResourceSamplerControl {
    inner: Arc<ResourceSamplerControlInner>,
}

#[derive(Default)]
struct ResourceSamplerControlInner {
    generation: AtomicU64,
    wake: tokio::sync::Notify,
}

impl ResourceSamplerControl {
    pub(super) fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    fn settings_changed(&self) {
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        self.inner.wake.notify_one();
    }
}

#[derive(Debug)]
struct SamplerState {
    enabled: bool,
    generation: u64,
    next_report: Option<Duration>,
    accumulator: ResourceUsageAccumulator,
}

impl SamplerState {
    fn new() -> Self {
        Self {
            enabled: false,
            generation: 0,
            next_report: None,
            accumulator: ResourceUsageAccumulator::new(),
        }
    }

    fn synchronize(&mut self, enabled: bool, generation: u64, now: Duration) {
        if self.enabled == enabled && self.generation == generation {
            return;
        }
        self.accumulator = ResourceUsageAccumulator::new();
        self.enabled = enabled;
        self.generation = generation;
        self.next_report = enabled.then_some(now + REPORT_INTERVAL);
    }

    fn accept(
        &mut self,
        generation: u64,
        observed_at: Duration,
        report_at: Duration,
        observation: Observation,
    ) -> Option<ResourceUsageSummary> {
        if !self.enabled || generation != self.generation {
            return None;
        }
        self.accumulator.observe(
            observed_at,
            observation.process,
            observation.database_bytes,
            observation.wal_bytes,
        );
        if !self
            .next_report
            .is_some_and(|deadline| report_at >= deadline)
        {
            return None;
        }
        self.next_report = Some(report_at + REPORT_INTERVAL);
        self.accumulator.take_summary()
    }
}

#[derive(Debug, Clone, Copy)]
struct Observation {
    process: ProcessSample,
    database_bytes: Option<u64>,
    wal_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct TimedObservation {
    sampled_at: tokio::time::Instant,
    observation: Observation,
}

pub(super) fn install(app: &tauri::AppHandle, schedulers: &Schedulers) {
    if !super::available() || super::environment_disabled() {
        return;
    }
    app.manage(ResourceSamplerControl::default());
    schedulers.push(tauri::async_runtime::spawn(run(app.clone())));
}

pub(super) fn settings_changed(app: &tauri::AppHandle) {
    if let Some(control) = app.try_state::<ResourceSamplerControl>() {
        control.settings_changed();
    }
}

async fn run(app: tauri::AppHandle) {
    let control = app.state::<ResourceSamplerControl>().inner().clone();
    let gate_app = app.clone();
    let reader_app = app.clone();
    let sink_app = app.clone();
    run_loop(
        control,
        move || super::allowed(&gate_app),
        move |generation| {
            let app = reader_app.clone();
            async move {
                let store = app.try_state::<Store>()?;
                let state_dir = store.state_dir().to_path_buf();
                tauri::async_runtime::spawn_blocking(move || {
                    let control = app.state::<ResourceSamplerControl>();
                    (super::allowed(&app) && control.generation() == generation).then(|| {
                        let sampled_at = tokio::time::Instant::now();
                        TimedObservation {
                            sampled_at,
                            observation: observe(&state_dir),
                        }
                    })
                })
                .await
                .ok()
                .flatten()
            }
        },
        move |generation, summary| {
            super::record_resource_usage(&sink_app, generation, summary);
        },
    )
    .await;
}

async fn run_loop<Gate, Read, ReadFuture, Sink>(
    control: ResourceSamplerControl,
    gate: Gate,
    read: Read,
    sink: Sink,
) where
    Gate: Fn() -> bool,
    Read: Fn(u64) -> ReadFuture,
    ReadFuture: Future<Output = Option<TimedObservation>>,
    Sink: Fn(u64, ResourceUsageSummary),
{
    let started = tokio::time::Instant::now();
    let mut state = SamplerState::new();
    loop {
        let generation = control.generation();
        let enabled = gate();
        state.synchronize(enabled, generation, started.elapsed());
        if !enabled {
            control.inner.wake.notified().await;
            continue;
        }

        let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = control.inner.wake.notified() => break,
                _ = interval.tick() => {}
            }

            if !gate() || generation != control.generation() {
                break;
            }
            let sample = read(generation);
            tokio::pin!(sample);
            let observation = tokio::select! {
                observation = &mut sample => observation,
                () = control.inner.wake.notified() => {
                    state.synchronize(false, control.generation(), started.elapsed());
                    let _ = sample.await;
                    break;
                }
            };
            let Some(observation) = observation else {
                continue;
            };
            if !gate() || generation != control.generation() {
                break;
            }
            if let Some(summary) = state.accept(
                generation,
                observation.sampled_at.duration_since(started),
                started.elapsed(),
                observation.observation,
            ) {
                sink(generation, summary);
            }
        }
    }
}

fn observe(state_dir: &Path) -> Observation {
    let database = database_path(state_dir);
    let wal = wal_path(&database);
    Observation {
        process: platform::sample(),
        database_bytes: file_bytes(&database, false),
        wal_bytes: file_bytes(&wal, true),
    }
}

fn wal_path(database: &Path) -> PathBuf {
    let mut path = database.as_os_str().to_owned();
    path.push("-wal");
    PathBuf::from(path)
}

fn file_bytes(path: &Path, missing_is_zero: bool) -> Option<u64> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Some(metadata.len()),
        Ok(_) => None,
        Err(error) if missing_is_zero && error.kind() == std::io::ErrorKind::NotFound => Some(0),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    fn observation(cpu_time_ns: u64) -> Observation {
        Observation {
            process: ProcessSample {
                cpu_time_ns: Some(cpu_time_ns),
                memory_bytes: Some(60 * 1024 * 1024),
                read_bytes: Some(cpu_time_ns),
                write_bytes: Some(cpu_time_ns),
            },
            database_bytes: Some(10),
            wal_bytes: Some(0),
        }
    }

    #[test]
    fn opt_out_rejects_late_samples_and_restarts_the_reporting_hour() {
        let mut state = SamplerState::new();
        state.synchronize(true, 1, Duration::ZERO);
        assert!(
            state
                .accept(1, Duration::ZERO, Duration::ZERO, observation(0))
                .is_none()
        );

        state.synchronize(false, 2, Duration::from_secs(30 * 60));
        assert!(
            state
                .accept(
                    1,
                    Duration::from_secs(60 * 60),
                    Duration::from_secs(60 * 60),
                    observation(1),
                )
                .is_none()
        );
        state.synchronize(true, 3, Duration::from_secs(60 * 60));
        assert!(
            state
                .accept(
                    3,
                    Duration::from_secs(119 * 60),
                    Duration::from_secs(119 * 60),
                    observation(2),
                )
                .is_none()
        );
        assert!(
            state
                .accept(
                    3,
                    Duration::from_secs(120 * 60),
                    Duration::from_secs(120 * 60),
                    observation(3),
                )
                .is_some()
        );
    }

    #[test]
    fn reports_at_most_once_per_hour_without_catch_up() {
        let mut state = SamplerState::new();
        state.synchronize(true, 1, Duration::ZERO);
        assert!(
            state
                .accept(1, REPORT_INTERVAL, REPORT_INTERVAL, observation(1))
                .is_some()
        );
        assert!(
            state
                .accept(
                    1,
                    REPORT_INTERVAL * 10,
                    REPORT_INTERVAL * 10,
                    observation(2),
                )
                .is_some()
        );
        assert!(
            state
                .accept(
                    1,
                    REPORT_INTERVAL * 10,
                    REPORT_INTERVAL * 10,
                    observation(3),
                )
                .is_none()
        );
    }

    #[test]
    fn missing_wal_is_zero_but_missing_database_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(file_bytes(&directory.path().join("db"), false), None);
        assert_eq!(file_bytes(&directory.path().join("db-wal"), true), Some(0));
    }

    #[tokio::test(start_paused = true)]
    async fn runtime_loop_gates_reads_fences_slow_work_and_keeps_its_cadence() {
        let control = ResourceSamplerControl::default();
        let enabled = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum_active = Arc::new(AtomicUsize::new(0));
        let reports = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(tokio::sync::Semaphore::new(0));

        let task = tokio::spawn(run_loop(
            control.clone(),
            {
                let enabled = enabled.clone();
                move || enabled.load(Ordering::Acquire)
            },
            {
                let reads = reads.clone();
                let active = active.clone();
                let maximum_active = maximum_active.clone();
                let permits = permits.clone();
                move |_| {
                    let reads = reads.clone();
                    let active = active.clone();
                    let maximum_active = maximum_active.clone();
                    let permits = permits.clone();
                    async move {
                        let sampled_at = tokio::time::Instant::now();
                        let index = reads.fetch_add(1, Ordering::AcqRel) + 1;
                        let now_active = active.fetch_add(1, Ordering::AcqRel) + 1;
                        maximum_active.fetch_max(now_active, Ordering::AcqRel);
                        let permit = permits.acquire().await.ok()?;
                        permit.forget();
                        active.fetch_sub(1, Ordering::AcqRel);
                        Some(TimedObservation {
                            sampled_at,
                            observation: observation(index as u64),
                        })
                    }
                }
            },
            {
                let reports = reports.clone();
                move |_, _| {
                    reports.fetch_add(1, Ordering::AcqRel);
                }
            },
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(REPORT_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(reads.load(Ordering::Acquire), 0);

        enabled.store(true, Ordering::Release);
        control.settings_changed();
        tokio::task::yield_now().await;
        assert_eq!(reads.load(Ordering::Acquire), 1);
        tokio::time::advance(SAMPLE_INTERVAL * 4).await;
        tokio::task::yield_now().await;
        assert_eq!(reads.load(Ordering::Acquire), 1);

        enabled.store(false, Ordering::Release);
        control.settings_changed();
        tokio::task::yield_now().await;
        permits.add_permits(1);
        tokio::task::yield_now().await;
        assert_eq!(reports.load(Ordering::Acquire), 0);

        enabled.store(true, Ordering::Release);
        control.settings_changed();
        permits.add_permits(100);
        tokio::task::yield_now().await;
        assert_eq!(reads.load(Ordering::Acquire), 2);
        tokio::time::advance(REPORT_INTERVAL - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(reports.load(Ordering::Acquire), 0);
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(reports.load(Ordering::Acquire), 1);

        tokio::time::advance(REPORT_INTERVAL * 8).await;
        tokio::task::yield_now().await;
        assert_eq!(reports.load(Ordering::Acquire), 2);
        assert_eq!(maximum_active.load(Ordering::Acquire), 1);

        let reads_before_abort = reads.load(Ordering::Acquire);
        task.abort();
        tokio::time::advance(REPORT_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(reads.load(Ordering::Acquire), reads_before_abort);
    }

    #[test]
    #[ignore = "reports local full-observation timing on demand"]
    fn reports_observation_latency() {
        let directory = tempfile::tempdir().unwrap();
        let database = database_path(directory.path());
        std::fs::File::create(&database)
            .unwrap()
            .set_len(64 * 1024)
            .unwrap();
        std::fs::File::create(wal_path(&database))
            .unwrap()
            .set_len(4 * 1024)
            .unwrap();

        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            std::hint::black_box(observe(directory.path()));
        }
        let elapsed = started.elapsed();
        eprintln!(
            "1000 full observations took {elapsed:?} ({:?} each)",
            elapsed / 1_000
        );
    }

    #[test]
    fn acquisition_time_controls_rates_when_completion_is_late() {
        use super::schema::CpuBand;

        let mut state = SamplerState::new();
        state.synchronize(true, 1, Duration::ZERO);
        state.accept(1, Duration::ZERO, Duration::ZERO, observation(0));
        let summary = state
            .accept(
                1,
                Duration::from_secs(5 * 60),
                REPORT_INTERVAL,
                observation(30 * 1_000_000_000),
            )
            .unwrap();

        assert_eq!(summary.cpu_average, CpuBand::From10ToUnder25Percent);
    }
}
