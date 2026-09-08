#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ProcessSample {
    pub cpu_time_ns: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub read_bytes: Option<u64>,
    pub write_bytes: Option<u64>,
}

#[cfg(target_os = "linux")]
pub(super) fn sample() -> ProcessSample {
    linux::sample()
}

#[cfg(target_os = "macos")]
pub(super) fn sample() -> ProcessSample {
    macos::sample()
}

#[cfg(target_os = "windows")]
pub(super) fn sample() -> ProcessSample {
    windows::sample()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) fn sample() -> ProcessSample {
    ProcessSample::default()
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    #[ignore = "reports local sampler timing on demand"]
    fn reports_sample_latency() {
        const SAMPLE_COUNT: u32 = 1_000;
        let started = Instant::now();
        for _ in 0..SAMPLE_COUNT {
            black_box(super::sample());
        }
        let elapsed = started.elapsed();
        eprintln!(
            "{SAMPLE_COUNT} process samples took {elapsed:?} ({:?} each)",
            elapsed / SAMPLE_COUNT
        );
    }
}
