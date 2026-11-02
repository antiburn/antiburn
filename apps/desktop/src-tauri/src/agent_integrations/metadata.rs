use std::collections::BTreeMap;
use std::ffi::{CStr, c_int, c_void};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::os::macos::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use super::transaction::{MAX_BYTES, Result};

unsafe extern "C" {
    fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut c_void;
    fn acl_size(acl: *mut c_void) -> isize;
    fn acl_copy_ext(buffer: *mut c_void, acl: *mut c_void, size: isize) -> isize;
    fn acl_free(object: *mut c_void) -> c_int;
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    identity: (u64, u64),
    owner: (u32, u32),
    mode: u32,
    flags: u32,
    links: u64,
    modified: (i64, i64),
    created: (i64, i64),
    acl: Vec<u8>,
    attributes: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Snapshot {
    pub(super) fn same_policy(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.mode == other.mode
            && self.flags == other.flags
            && self.acl == other.acl
            && self.attributes == other.attributes
    }
}

pub(super) fn snapshot(path: &Path) -> Result<Snapshot> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "config_metadata_unavailable")?;
    let metadata = file.metadata().map_err(|_| "config_metadata_unavailable")?;
    if !metadata.is_file() {
        return Err("unsafe_config");
    }
    let fd = file.as_raw_fd();
    let acl = acl_bytes(fd)?;
    let mut names = vec![0u8; MAX_BYTES as usize];
    let count = unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast(), names.len(), 0) };
    if count < 0 {
        return Err("config_metadata_unavailable");
    }
    names.truncate(count as usize);
    let mut attributes = BTreeMap::new();
    let mut remaining = MAX_BYTES as usize;
    for name in names.split_inclusive(|byte| *byte == 0) {
        let name = CStr::from_bytes_with_nul(name).map_err(|_| "config_metadata_unavailable")?;
        let mut value = vec![0u8; remaining];
        let count = unsafe {
            libc::fgetxattr(
                fd,
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        if count < 0 || count as usize > remaining {
            return Err("config_metadata_unavailable");
        }
        value.truncate(count as usize);
        remaining -= value.len();
        attributes.insert(name.to_bytes().to_vec(), value);
    }
    let after = file.metadata().map_err(|_| "config_metadata_unavailable")?;
    if (metadata.st_ctime(), metadata.st_ctime_nsec()) != (after.st_ctime(), after.st_ctime_nsec())
    {
        return Err("concurrent_config_change");
    }
    Ok(Snapshot {
        identity: (metadata.st_dev(), metadata.st_ino()),
        owner: (metadata.st_uid(), metadata.st_gid()),
        mode: metadata.st_mode(),
        flags: metadata.st_flags(),
        links: metadata.st_nlink(),
        modified: (metadata.st_mtime(), metadata.st_mtime_nsec()),
        created: (metadata.st_birthtime(), metadata.st_birthtime_nsec()),
        acl,
        attributes,
    })
}

fn acl_bytes(fd: c_int) -> Result<Vec<u8>> {
    // ACL_TYPE_EXTENDED comes from the macOS sys/acl.h ABI.
    let acl = unsafe { acl_get_fd_np(fd, 0x100) };
    if acl.is_null() {
        return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(Vec::new())
        } else {
            Err("config_metadata_unavailable")
        };
    }
    let result = (|| {
        let size = unsafe { acl_size(acl) };
        if size < 0 || size as u64 > MAX_BYTES {
            return Err("config_metadata_unavailable");
        }
        let mut bytes = vec![0u8; size as usize];
        let count = unsafe { acl_copy_ext(bytes.as_mut_ptr().cast(), acl, size) };
        if count < 0 || count > size {
            return Err("config_metadata_unavailable");
        }
        bytes.truncate(count as usize);
        Ok(bytes)
    })();
    unsafe { acl_free(acl) };
    result
}
