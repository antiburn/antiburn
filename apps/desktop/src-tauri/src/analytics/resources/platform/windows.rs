use std::mem::size_of;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
};

use super::ProcessSample;

pub(super) fn sample() -> ProcessSample {
    // SAFETY: GetCurrentProcess returns a pseudo handle that must not be closed.
    let process = unsafe { GetCurrentProcess() };
    let mut result = ProcessSample::default();

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: Each pointer refers to writable storage for the required FILETIME.
    if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } != 0 {
        result.cpu_time_ns = filetime_units(kernel)
            .checked_add(filetime_units(user))
            .and_then(|units| units.checked_mul(100));
    }

    let mut memory = PROCESS_MEMORY_COUNTERS {
        cb: u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).unwrap_or(0),
        ..Default::default()
    };
    let memory_size = memory.cb;
    // SAFETY: memory has the declared size and remains valid for the call.
    if memory_size != 0 && unsafe { GetProcessMemoryInfo(process, &mut memory, memory_size) } != 0 {
        result.memory_bytes = u64::try_from(memory.WorkingSetSize).ok();
    }

    let mut io = IO_COUNTERS::default();
    // SAFETY: io is valid writable storage for IO_COUNTERS.
    if unsafe { GetProcessIoCounters(process, &mut io) } != 0 {
        result.read_bytes = Some(io.ReadTransferCount);
        result.write_bytes = Some(io.WriteTransferCount);
    }

    result
}

fn filetime_units(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_filetime_halves() {
        assert_eq!(
            filetime_units(FILETIME {
                dwLowDateTime: 0x89ab_cdef,
                dwHighDateTime: 0x0123_4567,
            }),
            0x0123_4567_89ab_cdef
        );
    }

    #[test]
    fn native_sample_returns_supported_counters() {
        let sample = sample();
        assert!(sample.cpu_time_ns.is_some());
        assert!(sample.memory_bytes.is_some());
        assert!(sample.read_bytes.is_some());
        assert!(sample.write_bytes.is_some());
    }
}
