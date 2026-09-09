use std::time::Duration;

use super::platform::ProcessSample;
use super::schema::{
    CpuBand, IoRateBand, MemoryBand, ResourceUsageSummary, coverage, cpu_band, io_rate_band,
    memory_band,
};

pub const MAX_VALID_SAMPLE_GAP: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy)]
struct Baseline {
    at: Duration,
    process: ProcessSample,
}

#[derive(Debug, Default)]
pub struct ResourceUsageAccumulator {
    baseline: Option<Baseline>,
    samples: u64,
    memory_samples: u64,
    memory_sum: u128,
    memory_max: u64,
    intervals: u64,
    cpu_intervals: u64,
    cpu_ns: u128,
    cpu_elapsed_ns: u128,
    read_intervals: u64,
    read_bytes: u128,
    read_elapsed_ns: u128,
    write_intervals: u64,
    write_bytes: u128,
    write_elapsed_ns: u128,
    database_samples: u64,
    database_bytes: Option<u64>,
    wal_samples: u64,
    wal_bytes: Option<u64>,
}

impl ResourceUsageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(
        &mut self,
        now: Duration,
        process: ProcessSample,
        database_bytes: Option<u64>,
        wal_bytes: Option<u64>,
    ) {
        self.samples += 1;
        if let Some(bytes) = process.memory_bytes {
            self.memory_samples += 1;
            self.memory_sum += u128::from(bytes);
            self.memory_max = self.memory_max.max(bytes);
        }

        if let Some(bytes) = database_bytes {
            self.database_samples += 1;
            self.database_bytes = Some(bytes);
        }
        if let Some(bytes) = wal_bytes {
            self.wal_samples += 1;
            self.wal_bytes = Some(bytes);
        }

        if let Some(previous) = self.baseline {
            self.intervals += 1;
            if let Some(elapsed) = now.checked_sub(previous.at)
                && !elapsed.is_zero()
                && elapsed <= MAX_VALID_SAMPLE_GAP
            {
                let elapsed_ns = elapsed.as_nanos();
                if let Some(delta) = previous
                    .process
                    .cpu_time_ns
                    .zip(process.cpu_time_ns)
                    .and_then(|(old, new)| new.checked_sub(old))
                {
                    self.cpu_intervals += 1;
                    self.cpu_ns += u128::from(delta);
                    self.cpu_elapsed_ns += elapsed_ns;
                }
                accumulate_io(
                    previous.process.read_bytes,
                    process.read_bytes,
                    elapsed_ns,
                    &mut self.read_intervals,
                    &mut self.read_bytes,
                    &mut self.read_elapsed_ns,
                );
                accumulate_io(
                    previous.process.write_bytes,
                    process.write_bytes,
                    elapsed_ns,
                    &mut self.write_intervals,
                    &mut self.write_bytes,
                    &mut self.write_elapsed_ns,
                );
            }
        }

        self.baseline = Some(Baseline { at: now, process });
    }

    pub fn take_summary(&mut self) -> Option<ResourceUsageSummary> {
        if self.samples == 0 {
            return None;
        }

        let summary = ResourceUsageSummary {
            memory_mean: if self.memory_samples == 0 {
                MemoryBand::Unavailable
            } else {
                memory_band(self.memory_sum / u128::from(self.memory_samples))
            },
            memory_max: if self.memory_samples == 0 {
                MemoryBand::Unavailable
            } else {
                memory_band(u128::from(self.memory_max))
            },
            memory_coverage: coverage(self.memory_samples, self.samples),
            cpu_average: if self.cpu_intervals == 0 {
                CpuBand::Unavailable
            } else {
                cpu_band(self.cpu_ns, self.cpu_elapsed_ns)
            },
            cpu_coverage: coverage(self.cpu_intervals, self.intervals),
            read_rate_average: if self.read_intervals == 0 {
                IoRateBand::Unavailable
            } else {
                io_rate_band(self.read_bytes, self.read_elapsed_ns)
            },
            read_coverage: coverage(self.read_intervals, self.intervals),
            write_rate_average: if self.write_intervals == 0 {
                IoRateBand::Unavailable
            } else {
                io_rate_band(self.write_bytes, self.write_elapsed_ns)
            },
            write_coverage: coverage(self.write_intervals, self.intervals),
            database_size: self
                .database_bytes
                .map_or(MemoryBand::Unavailable, |bytes| memory_band(bytes.into())),
            database_coverage: coverage(self.database_samples, self.samples),
            wal_size: self
                .wal_bytes
                .map_or(MemoryBand::Unavailable, |bytes| memory_band(bytes.into())),
            wal_coverage: coverage(self.wal_samples, self.samples),
        };
        self.reset_window();
        Some(summary)
    }

    fn reset_window(&mut self) {
        let baseline = self.baseline;
        *self = Self::default();
        self.baseline = baseline;
    }
}

fn accumulate_io(
    previous: Option<u64>,
    current: Option<u64>,
    elapsed_ns: u128,
    intervals: &mut u64,
    bytes: &mut u128,
    elapsed_total_ns: &mut u128,
) {
    let Some(delta) = previous
        .zip(current)
        .and_then(|(old, new)| new.checked_sub(old))
    else {
        return;
    };
    *intervals += 1;
    *bytes += u128::from(delta);
    *elapsed_total_ns += elapsed_ns;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::resources::schema::CoverageBand;

    fn sample(
        cpu_time_ns: Option<u64>,
        memory_bytes: Option<u64>,
        read_bytes: Option<u64>,
        write_bytes: Option<u64>,
    ) -> ProcessSample {
        ProcessSample {
            cpu_time_ns,
            memory_bytes,
            read_bytes,
            write_bytes,
        }
    }

    #[test]
    fn first_sample_is_only_a_counter_baseline() {
        let mut aggregate = ResourceUsageAccumulator::new();
        aggregate.observe(
            Duration::from_secs(5),
            sample(Some(10), Some(0), Some(20), Some(30)),
            Some(0),
            None,
        );

        let summary = aggregate.take_summary().expect("one sample");
        assert_eq!(summary.memory_mean, MemoryBand::Under50Mib);
        assert_eq!(summary.cpu_average, CpuBand::Unavailable);
        assert_eq!(summary.cpu_coverage, CoverageBand::None);
        assert_eq!(summary.read_rate_average, IoRateBand::Unavailable);
        assert_eq!(summary.database_size, MemoryBand::Under50Mib);
        assert_eq!(summary.wal_size, MemoryBand::Unavailable);
    }

    #[test]
    fn averages_deltas_over_their_measured_elapsed_time() {
        let mut aggregate = ResourceUsageAccumulator::new();
        aggregate.observe(
            Duration::ZERO,
            sample(Some(0), Some(50 * 1024 * 1024), Some(0), Some(0)),
            None,
            None,
        );
        aggregate.observe(
            Duration::from_secs(10),
            sample(
                Some(1_000_000_000),
                Some(100 * 1024 * 1024),
                Some(1024),
                Some(0),
            ),
            None,
            None,
        );
        aggregate.observe(
            Duration::from_secs(30),
            sample(
                Some(2_000_000_000),
                Some(250 * 1024 * 1024),
                Some(29 * 1024),
                Some(0),
            ),
            None,
            None,
        );

        let summary = aggregate.take_summary().expect("samples");
        assert_eq!(summary.cpu_average, CpuBand::From5ToUnder10Percent);
        assert_eq!(summary.cpu_coverage, CoverageBand::Full);
        assert_eq!(summary.read_rate_average, IoRateBand::Under1KibPerSecond);
        assert_eq!(summary.read_coverage, CoverageBand::Full);
        assert_eq!(summary.write_rate_average, IoRateBand::Zero);
        assert_eq!(summary.memory_mean, MemoryBand::From100ToUnder250Mib);
        assert_eq!(summary.memory_max, MemoryBand::From250ToUnder500Mib);
    }

    #[test]
    fn missing_counters_and_resets_never_become_zero_usage() {
        let mut aggregate = ResourceUsageAccumulator::new();
        aggregate.observe(
            Duration::ZERO,
            sample(Some(100), None, Some(100), None),
            None,
            None,
        );
        aggregate.observe(
            Duration::from_secs(300),
            sample(Some(50), None, None, Some(0)),
            None,
            None,
        );

        let summary = aggregate.take_summary().expect("samples");
        assert_eq!(summary.memory_mean, MemoryBand::Unavailable);
        assert_eq!(summary.memory_coverage, CoverageBand::None);
        assert_eq!(summary.cpu_average, CpuBand::Unavailable);
        assert_eq!(summary.read_rate_average, IoRateBand::Unavailable);
        assert_eq!(summary.write_rate_average, IoRateBand::Unavailable);
    }

    #[test]
    fn zero_elapsed_and_suspend_gaps_are_invalid_intervals() {
        let mut aggregate = ResourceUsageAccumulator::new();
        aggregate.observe(
            Duration::ZERO,
            sample(Some(0), Some(1), Some(0), Some(0)),
            None,
            None,
        );
        aggregate.observe(
            Duration::ZERO,
            sample(Some(1), Some(1), Some(1), Some(1)),
            None,
            None,
        );
        aggregate.observe(
            MAX_VALID_SAMPLE_GAP + Duration::from_secs(1),
            sample(Some(2), Some(1), Some(2), Some(2)),
            None,
            None,
        );

        let summary = aggregate.take_summary().expect("samples");
        assert_eq!(summary.cpu_coverage, CoverageBand::None);
        assert_eq!(summary.read_coverage, CoverageBand::None);
    }

    #[test]
    fn taking_a_summary_keeps_the_counter_baseline() {
        let mut aggregate = ResourceUsageAccumulator::new();
        aggregate.observe(
            Duration::ZERO,
            sample(Some(0), Some(1), Some(0), Some(0)),
            None,
            None,
        );
        aggregate.take_summary().expect("first window");
        aggregate.observe(
            Duration::from_secs(300),
            sample(Some(30_000_000_000), Some(1), Some(1024), Some(0)),
            None,
            None,
        );

        let summary = aggregate.take_summary().expect("second window");
        assert_eq!(summary.cpu_average, CpuBand::From10ToUnder25Percent);
        assert_eq!(summary.cpu_coverage, CoverageBand::Full);
    }

    #[test]
    fn accumulator_has_fixed_small_storage() {
        assert!(std::mem::size_of::<ResourceUsageAccumulator>() <= 384);
    }
}
