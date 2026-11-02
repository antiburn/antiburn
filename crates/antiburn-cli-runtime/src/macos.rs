use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Result, ensure};

unsafe extern "C" {
    fn acl_get_file(path: *const c_char, acl_type: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    fn acl_get_permset_mask_np(entry: *mut c_void, mask: *mut u64) -> c_int;
    fn acl_get_qualifier(entry: *mut c_void) -> *mut c_void;
    fn mbr_uuid_to_id(uuid: *const u8, id: *mut u32, kind: *mut c_int) -> c_int;
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

pub fn check_ancestor(path: &Path) -> Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    let acl = unsafe { acl_get_file(path.as_ptr(), 0x100) };
    if acl.is_null() {
        ensure!(
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT),
            "Cannot inspect CLI ancestor ACL"
        );
        return Ok(());
    }
    let result = (|| -> Result<()> {
        let mut entry = std::ptr::null_mut();
        let mut entry_id = 0;
        loop {
            if unsafe { acl_get_entry(acl, entry_id, &mut entry) } != 0 {
                ensure!(
                    std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL),
                    "Cannot read CLI ancestor ACL entry"
                );
                break;
            }
            entry_id = -1;
            let mut tag = 0;
            ensure!(
                unsafe { acl_get_tag_type(entry, &mut tag) } == 0,
                "Cannot read CLI ancestor ACL type"
            );
            if tag == 2 {
                continue;
            }
            ensure!(tag == 1, "CLI ancestor ACL type is unsupported");
            let mut mask = 0;
            ensure!(
                unsafe { acl_get_permset_mask_np(entry, &mut mask) } == 0,
                "Cannot read CLI ancestor ACL permissions"
            );
            // These rights can replace children or change the directory policy.
            let writes = (1 << 2)
                | (1 << 4)
                | (1 << 5)
                | (1 << 6)
                | (1 << 8)
                | (1 << 10)
                | (1 << 12)
                | (1 << 13);
            if mask & writes == 0 {
                continue;
            }
            let qualifier = unsafe { acl_get_qualifier(entry) };
            ensure!(
                !qualifier.is_null(),
                "Cannot read CLI ancestor ACL principal"
            );
            let mut id = 0;
            let mut kind = -1;
            let resolved = unsafe { mbr_uuid_to_id(qualifier.cast(), &mut id, &mut kind) };
            unsafe {
                acl_free(qualifier);
            }
            ensure!(
                resolved == 0 && kind == 0 && (id == 0 || id == unsafe { libc::geteuid() }),
                "CLI ancestor ACL permits unrelated writes; its policy was preserved"
            );
        }
        Ok(())
    })();
    unsafe {
        acl_free(acl);
    }
    result
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
