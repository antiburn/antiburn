use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Result, ensure};

unsafe extern "C" {
    fn acl_get_file(path: *const c_char, acl_type: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_free(object: *mut c_void) -> c_int;
    fn fcopyfile(from: c_int, to: c_int, state: *mut c_void, flags: u32) -> c_int;
}

pub fn check_policy(path: &Path) -> Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // These constants come from the macOS sys/acl.h ABI.
    let acl = unsafe { acl_get_file(path.as_ptr(), 0x100) };
    if acl.is_null() {
        ensure!(
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT),
            "Cannot inspect CLI path ACL: {}",
            std::io::Error::last_os_error()
        );
        return Ok(());
    }
    let mut entry = std::ptr::null_mut();
    let result = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = std::io::Error::last_os_error();
    unsafe {
        acl_free(acl);
    }
    ensure!(
        result == -1 && error.raw_os_error() == Some(libc::EINVAL),
        "CLI path has an extended ACL; its policy was preserved"
    );
    Ok(())
}

pub fn copy_attributes(source: &Path, destination: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let source_path = source;
    let source = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)?;
    let result = unsafe {
        fcopyfile(
            source.as_raw_fd(),
            destination.as_raw_fd(),
            std::ptr::null_mut(),
            1 << 2,
        )
    };
    ensure!(
        result == 0,
        "Cannot preserve CLI extended attributes: {}",
        std::io::Error::last_os_error()
    );
    preserve_bundle_quarantine(source_path, destination)
}

fn preserve_bundle_quarantine(source: &Path, destination: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let name = c"com.apple.quarantine";
    let existing = unsafe {
        libc::fgetxattr(
            destination.as_raw_fd(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if existing >= 0 {
        return Ok(());
    }
    ensure!(
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOATTR),
        "Cannot inspect staged CLI quarantine"
    );
    let Some(bundle) = source
        .ancestors()
        .take(6)
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
    else {
        return Ok(());
    };
    let bundle = CString::new(bundle.as_os_str().as_bytes())?;
    let mut value = [0u8; 4096];
    let length = unsafe {
        libc::getxattr(
            bundle.as_ptr(),
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if length < 0 {
        ensure!(
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOATTR),
            "Cannot preserve app bundle quarantine"
        );
        return Ok(());
    }
    ensure!(
        unsafe {
            libc::fsetxattr(
                destination.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast(),
                length as usize,
                0,
                0,
            )
        } == 0,
        "Cannot preserve app bundle quarantine on CLI"
    );
    Ok(())
}
