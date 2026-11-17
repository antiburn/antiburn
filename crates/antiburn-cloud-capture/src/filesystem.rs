//! Source admission and file-generation checks without payload reads.

#[cfg(unix)]
use crate::{Error, Result};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::path::{Component, Path};

#[cfg(unix)]
/// Open each absolute path component without following symlinks.
pub(crate) fn open_source(path: &Path, directory: bool) -> Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    if !path.is_absolute() {
        return Err(Error::InvalidSource);
    }
    let mut current = File::open("/")?;
    let components: Vec<_> = path.components().collect();
    for (index, component) in components.iter().enumerate() {
        if matches!(component, Component::RootDir) {
            continue;
        }
        let Component::Normal(name) = component else {
            return Err(Error::InvalidSource);
        };
        let name = CString::new(name.as_bytes()).map_err(|_| Error::InvalidSource)?;
        let is_directory = index + 1 != components.len() || directory;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_directory { libc::O_DIRECTORY } else { 0 };
        // Each open stays relative to the admitted directory handle.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(Error::Io(std::io::Error::last_os_error().kind()));
        }
        // openat returns a new descriptor with one owner.
        current = unsafe { File::from_raw_fd(fd) };
    }
    let metadata = current.metadata()?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(Error::InvalidSource);
    }
    Ok(current)
}

#[cfg(unix)]
/// Compare pinned identity, size, timestamps, and one remaining hard link.
pub(crate) fn same_source(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
        && after.nlink() == 1
}

#[cfg(not(unix))]
/// Native file generations are unavailable on unsupported platforms.
pub(crate) fn same_source(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    false
}
