//! Prove that a registered Unix process has the selected database file open.

use std::fmt;
use std::fs;
use std::path::Path;

const MAX_PROCESS_DESCRIPTORS: usize = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FileIdentity {
    pub(super) device: u64,
    pub(super) inode: u64,
}

impl FileIdentity {
    fn matches(self, other: Self) -> bool {
        self == other
    }
}

#[derive(Debug)]
pub struct ProofError(&'static str);

impl fmt::Display for ProofError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ProofError {}

/// Return true only when `pid` has the exact regular database file open.
///
/// This compares device and inode values. It does not trust a process's file
/// path, which can be stale, renamed, or replaced.
pub fn process_has_open_database(pid: u32, database_path: &Path) -> Result<bool, ProofError> {
    let database =
        fs::metadata(database_path).map_err(|_| ProofError("cannot inspect database file"))?;
    if !database.is_file() {
        return Err(ProofError("database path is not a regular file"));
    }
    let target = identity(&database);

    let before = process_start_identity(pid)?;
    let matched = process_has_identity(pid, target);
    let after = process_start_identity(pid)?;

    verify_enumeration(before, after, matched)
}

fn verify_enumeration(
    before: u128,
    after: u128,
    matched: Result<bool, ProofError>,
) -> Result<bool, ProofError> {
    if before != after {
        return Err(ProofError("process identity changed during enumeration"));
    }

    matched
}

#[cfg(unix)]
pub(super) fn identity(metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(target_os = "linux")]
fn process_start_identity(pid: u32) -> Result<u128, ProofError> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|_| ProofError("cannot read process identity"))?;
    // The command name is parenthesized and can contain spaces or parentheses.
    let mut fields = stat
        .rsplit_once(") ")
        .ok_or(ProofError("invalid process identity"))?
        .1
        .split_whitespace();
    // The first field after comm is field 3; starttime is field 22.
    let start_ticks = fields
        .nth(19)
        .ok_or(ProofError("invalid process identity"))?
        .parse::<u64>()
        .map_err(|_| ProofError("invalid process identity"))?;
    Ok(u128::from(start_ticks))
}

#[cfg(target_os = "linux")]
fn process_has_identity(pid: u32, target: FileIdentity) -> Result<bool, ProofError> {
    let entries = fs::read_dir(format!("/proc/{pid}/fd"))
        .map_err(|_| ProofError("cannot enumerate process file descriptors"))?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_PROCESS_DESCRIPTORS {
            return Err(ProofError("file descriptor list exceeds its limit"));
        }
        let entry = entry.map_err(|_| ProofError("incomplete file descriptor enumeration"))?;
        let metadata = match fs::metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if vanished_descriptor(&error) => continue,
            Err(_) => return Err(ProofError("cannot inspect process file descriptor")),
        };
        if metadata.is_file() && identity(&metadata).matches(target) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn vanished_descriptor(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOENT | libc::ESRCH | libc::EBADF)
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{FileIdentity, MAX_PROCESS_DESCRIPTORS, ProofError};
    use std::ffi::c_void;
    use std::mem::size_of;

    const PROC_PIDLISTFDS: i32 = 1;
    const PROC_PIDTBSDINFO: i32 = 3;
    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    const PROX_FDTYPE_VNODE: u32 = 1;
    const MAXPATHLEN: usize = 1024;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcFdInfo {
        fd: i32,
        fd_type: u32,
    }

    #[repr(C)]
    struct ProcBsdInfo {
        _flags: u32,
        _status: u32,
        _xstatus: u32,
        pid: u32,
        _ppid: u32,
        _uid: u32,
        _gid: u32,
        _ruid: u32,
        _rgid: u32,
        _svuid: u32,
        _svgid: u32,
        _reserved: u32,
        _comm: [u8; 16],
        _name: [u8; 32],
        _nfiles: u32,
        _pgid: u32,
        _pjobc: u32,
        _tty_device: u32,
        _tty_pgid: u32,
        _nice: i32,
        start_seconds: u64,
        start_microseconds: u64,
    }

    #[repr(C)]
    struct VinfoStat {
        device: u32,
        mode: u16,
        _links: u16,
        inode: u64,
        _uid: u32,
        _gid: u32,
        _access_seconds: i64,
        _access_nanoseconds: i64,
        _modify_seconds: i64,
        _modify_nanoseconds: i64,
        _change_seconds: i64,
        _change_nanoseconds: i64,
        _birth_seconds: i64,
        _birth_nanoseconds: i64,
        _size: i64,
        _blocks: i64,
        _block_size: i32,
        _flags: u32,
        _generation: u32,
        _raw_device: u32,
        _reserved: [i64; 2],
    }

    #[repr(C)]
    struct VnodeInfo {
        stat: VinfoStat,
        _vnode_type: i32,
        _padding: i32,
        _filesystem_id: [i32; 2],
    }

    #[repr(C)]
    struct VnodeInfoPath {
        vnode: VnodeInfo,
        _path: [u8; MAXPATHLEN],
    }

    #[repr(C)]
    struct ProcFileInfo {
        _open_flags: u32,
        _status: u32,
        _offset: i64,
        _type: i32,
        _guard_flags: u32,
    }

    #[repr(C)]
    struct VnodeFdInfoWithPath {
        _file: ProcFileInfo,
        vnode_path: VnodeInfoPath,
    }

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, size: i32) -> i32;
        fn proc_pidfdinfo(pid: i32, fd: i32, flavor: i32, buffer: *mut c_void, size: i32) -> i32;
    }

    pub(super) fn process_start_identity(pid: u32) -> Result<u128, ProofError> {
        let mut info = std::mem::MaybeUninit::<ProcBsdInfo>::uninit();
        // SAFETY: `info` points to writable storage of the exact ABI size.
        let result = unsafe {
            proc_pidinfo(
                pid as i32,
                PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size_of::<ProcBsdInfo>() as i32,
            )
        };
        if result != size_of::<ProcBsdInfo>() as i32 {
            return Err(ProofError("cannot read process identity"));
        }
        // SAFETY: libproc returned the full structure above.
        let info = unsafe { info.assume_init() };
        if info.pid != pid {
            return Err(ProofError("process identity does not match PID"));
        }
        Ok((u128::from(info.start_seconds) << 64) | u128::from(info.start_microseconds))
    }

    pub(super) fn process_has_identity(pid: u32, target: FileIdentity) -> Result<bool, ProofError> {
        let pid = pid as i32;
        let mut capacity = 64usize;
        let descriptors = loop {
            if capacity > MAX_PROCESS_DESCRIPTORS {
                return Err(ProofError("file descriptor list exceeds its limit"));
            }
            let bytes = capacity
                .checked_mul(size_of::<ProcFdInfo>())
                .filter(|bytes| *bytes <= i32::MAX as usize)
                .ok_or(ProofError("file descriptor list is too large"))?;
            let mut buffer = vec![0u64; bytes.div_ceil(size_of::<u64>())];
            // SAFETY: the buffer is writable and has at least `bytes` bytes.
            let result = unsafe {
                proc_pidinfo(
                    pid,
                    PROC_PIDLISTFDS,
                    0,
                    buffer.as_mut_ptr().cast(),
                    bytes as i32,
                )
            };
            if result < 0 || !(result as usize).is_multiple_of(size_of::<ProcFdInfo>()) {
                return Err(ProofError("cannot enumerate process file descriptors"));
            }
            if (result as usize) < bytes {
                // SAFETY: libproc returned a whole number of initialized entries.
                let descriptors = unsafe {
                    std::slice::from_raw_parts(
                        buffer.as_ptr().cast::<ProcFdInfo>(),
                        result as usize / size_of::<ProcFdInfo>(),
                    )
                };
                break descriptors.to_vec();
            }
            capacity = capacity
                .checked_mul(2)
                .ok_or(ProofError("file descriptor list is too large"))?;
        };

        for descriptor in descriptors {
            if descriptor.fd_type != PROX_FDTYPE_VNODE {
                continue;
            }
            let mut info = std::mem::MaybeUninit::<VnodeFdInfoWithPath>::uninit();
            // SAFETY: `info` points to writable storage of the exact ABI size.
            let result = unsafe {
                proc_pidfdinfo(
                    pid,
                    descriptor.fd,
                    PROC_PIDFDVNODEPATHINFO,
                    info.as_mut_ptr().cast(),
                    size_of::<VnodeFdInfoWithPath>() as i32,
                )
            };
            if result != size_of::<VnodeFdInfoWithPath>() as i32 {
                if result <= 0 && super::vanished_descriptor(&std::io::Error::last_os_error()) {
                    continue;
                }
                return Err(ProofError("incomplete vnode descriptor enumeration"));
            }
            // SAFETY: libproc returned the full structure above.
            let info = unsafe { info.assume_init() };
            let stat = info.vnode_path.vnode.stat;
            if stat.mode & 0o170000 == 0o100000 {
                let identity = FileIdentity {
                    device: u64::from(stat.device),
                    inode: stat.inode,
                };
                if identity.matches(target) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

#[cfg(target_os = "macos")]
use macos::{process_has_identity, process_start_identity};

#[cfg(test)]
mod tests {
    use super::{FileIdentity, ProofError, verify_enumeration};

    #[test]
    fn identity_requires_device_and_inode_match() {
        let target = FileIdentity {
            device: 7,
            inode: 42,
        };
        assert!(target.matches(target));
        assert!(!target.matches(FileIdentity {
            device: 8,
            inode: 42,
        }));
        assert!(!target.matches(FileIdentity {
            device: 7,
            inode: 43,
        }));
    }

    #[test]
    fn process_reuse_fails_closed() {
        assert!(verify_enumeration(10, 11, Ok(true)).is_err());
    }

    #[test]
    fn incomplete_enumeration_fails_closed() {
        assert!(verify_enumeration(10, 10, Err(ProofError("incomplete enumeration")),).is_err());
    }

    #[test]
    fn unrelated_descriptor_churn_does_not_change_positive_proof() {
        assert!(verify_enumeration(10, 10, Ok(true)).unwrap());
        for errno in [libc::ENOENT, libc::ESRCH, libc::EBADF] {
            assert!(super::vanished_descriptor(
                &std::io::Error::from_raw_os_error(errno)
            ));
        }
        assert!(!super::vanished_descriptor(
            &std::io::Error::from_raw_os_error(libc::EACCES)
        ));
    }
}
