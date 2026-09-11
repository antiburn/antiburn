use std::mem::MaybeUninit;

use super::ProcessSample;

const RUSAGE_INFO_V2: i32 = 2;

#[repr(C)]
struct RusageInfoV2 {
    _uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    _package_idle_wakeups: u64,
    _interrupt_wakeups: u64,
    _pageins: u64,
    _wired_size: u64,
    _resident_size: u64,
    physical_footprint: u64,
    _process_start_absolute_time: u64,
    _process_exit_absolute_time: u64,
    _child_user_time: u64,
    _child_system_time: u64,
    _child_package_idle_wakeups: u64,
    _child_interrupt_wakeups: u64,
    _child_pageins: u64,
    _child_elapsed_absolute_time: u64,
    disk_io_bytes_read: u64,
    disk_io_bytes_written: u64,
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV2) -> i32;
}

pub(super) fn sample() -> ProcessSample {
    let Ok(pid) = i32::try_from(std::process::id()) else {
        return ProcessSample::default();
    };
    let mut usage = MaybeUninit::<RusageInfoV2>::uninit();
    // SAFETY: RUSAGE_INFO_V2 selects the exact repr(C) buffer layout above.
    let result = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, usage.as_mut_ptr()) };
    if result != 0 {
        return ProcessSample::default();
    }
    // SAFETY: proc_pid_rusage initializes the complete buffer when it succeeds.
    let usage = unsafe { usage.assume_init() };

    ProcessSample {
        cpu_time_ns: usage.user_time.checked_add(usage.system_time),
        memory_bytes: Some(usage.physical_footprint),
        read_bytes: Some(usage.disk_io_bytes_read),
        write_bytes: Some(usage.disk_io_bytes_written),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sample_returns_supported_counters() {
        let sample = sample();
        assert!(sample.cpu_time_ns.is_some());
        assert!(sample.memory_bytes.is_some());
        assert!(sample.read_bytes.is_some());
        assert!(sample.write_bytes.is_some());
    }
}
