use crate::{Error, RECORD_BYTES, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[cfg(unix)]
use std::ffi::{CStr, CString};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[cfg(unix)]
#[derive(Debug)]
pub(crate) struct Directory {
    file: File,
}

#[cfg(unix)]
impl Directory {
    pub fn source(path: &Path) -> Result<Self> {
        Ok(Self {
            file: crate::filesystem::open_source(path, true)?,
        })
    }

    pub fn source_dir(&self, name: &str) -> Result<Self> {
        Ok(Self {
            file: self.open_at(name, libc::O_RDONLY | libc::O_DIRECTORY)?,
        })
    }

    pub fn source_file(&self, name: &str) -> Result<File> {
        let file = self.open_at(name, libc::O_RDONLY)?;
        if !file.metadata()?.is_file() {
            return Err(Error::InvalidSource);
        }
        Ok(file)
    }

    pub fn kind(&self, name: &str) -> Result<EntryKind> {
        let name = component(name)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stat = unsafe { stat.assume_init() };
        Ok(match stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => EntryKind::File,
            libc::S_IFDIR => EntryKind::Directory,
            _ => EntryKind::Other,
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let parent = path.parent().ok_or(Error::InvalidSource)?;
        let name = path.file_name().ok_or(Error::InvalidSource)?;
        if parent.join(name) != path {
            return Err(Error::InvalidSource);
        }
        let parent = Self {
            file: crate::filesystem::open_source(parent, true)?,
        };
        let name = name.to_str().ok_or(Error::InvalidSource)?;
        parent.create_dir(name)
    }

    pub fn create_dir(&self, name: &str) -> Result<Self> {
        let name_c = component(name)?;
        let result = unsafe { libc::mkdirat(self.file.as_raw_fd(), name_c.as_ptr(), 0o700) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        } else {
            self.sync()?;
        }
        self.open_dir(name)
    }

    pub fn open_dir(&self, name: &str) -> Result<Self> {
        let file = self.open_at(name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let metadata = file.metadata()?;
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_dir()
            || metadata.permissions().mode() & 0o777 != 0o700
            || metadata.uid() != owner
        {
            return Err(Error::InvalidSource);
        }
        Ok(Self { file })
    }

    pub fn file(&self, name: &str, create: bool) -> Result<File> {
        let flags = if create {
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY
        };
        let file = self.open_at(name, flags)?;
        let metadata = file.metadata()?;
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.uid() != owner
            || metadata.nlink() != 1
        {
            return Err(Error::CorruptSpool);
        }
        Ok(file)
    }

    fn open_at(&self, name: &str, flags: i32) -> Result<File> {
        let name = component(name)?;
        let flags = flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let fd = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // openat returns a new descriptor with one owner.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub fn names(&self, maximum: usize) -> Result<Vec<String>> {
        // A new open file description keeps iteration offsets independent.
        let dot = CString::new(".").map_err(|_| Error::InvalidSource)?;
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let raw = unsafe { libc::fdopendir(file.as_raw_fd()) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        // fdopendir owns this descriptor until closedir runs.
        let _ = file.into_raw_fd();
        let entries = Entries(raw);
        let mut names = Vec::new();
        loop {
            errno::set_errno(errno::Errno(0));
            let entry = unsafe { libc::readdir(entries.0) };
            if entry.is_null() {
                let code = errno::errno().0;
                if code != 0 {
                    return Err(std::io::Error::from_raw_os_error(code).into());
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if names.len() >= maximum {
                return Err(Error::Limit);
            }
            names.push(
                std::str::from_utf8(name)
                    .map_err(|_| Error::CorruptSpool)?
                    .to_owned(),
            );
        }
        Ok(names)
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        let from = component(from)?;
        let to = component(to)?;
        let result = unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.sync()
    }

    pub fn remove(&self, name: &str, directory: bool) -> Result<()> {
        let name = component(name)?;
        let result = unsafe {
            libc::unlinkat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub fn sync(&self) -> Result<()> {
        Ok(self.file.sync_all()?)
    }
}

#[cfg(unix)]
fn component(value: &str) -> Result<CString> {
    if value.is_empty() || value == "." || value == ".." || value.contains('/') {
        return Err(Error::InvalidSource);
    }
    CString::new(value.as_bytes()).map_err(|_| Error::InvalidSource)
}

#[cfg(unix)]
struct Entries(*mut libc::DIR);

#[cfg(unix)]
impl Drop for Entries {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

#[cfg(not(unix))]
#[derive(Debug)]
pub(crate) struct Directory;

#[cfg(not(unix))]
impl Directory {
    pub fn source(_: &Path) -> Result<Self> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn source_dir(&self, _: &str) -> Result<Self> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn source_file(&self, _: &str) -> Result<File> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn kind(&self, _: &str) -> Result<EntryKind> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn open(_: &Path) -> Result<Self> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn create_dir(&self, _: &str) -> Result<Self> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn open_dir(&self, _: &str) -> Result<Self> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn file(&self, _: &str, _: bool) -> Result<File> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn names(&self, _: usize) -> Result<Vec<String>> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn rename(&self, _: &str, _: &str) -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn remove(&self, _: &str, _: bool) -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }
    pub fn sync(&self) -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }
}

#[derive(PartialEq, Eq)]
pub(crate) enum EntryKind {
    File,
    Directory,
    #[cfg(unix)]
    Other,
}

impl Directory {
    pub fn read_bounded(&self, name: &str, maximum: u64) -> Result<Vec<u8>> {
        let file = self.file(name, false)?;
        if file.metadata()?.len() > maximum {
            return Err(Error::CorruptSpool);
        }
        let mut bytes = Vec::new();
        file.take(maximum + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > maximum {
            return Err(Error::CorruptSpool);
        }
        Ok(bytes)
    }

    pub fn save_record(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > RECORD_BYTES {
            return Err(Error::Limit);
        }
        match self.remove("record.next", false) {
            Ok(()) => {}
            Err(Error::Io(std::io::ErrorKind::NotFound)) => {}
            Err(error) => return Err(error),
        }
        let mut file = self.file("record.next", true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.rename("record.next", "record.json")
    }
}
