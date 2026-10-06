//! Descriptor-based admission and request work limits for remote evidence.

#[cfg(unix)]
use std::ffi::CString;
use std::fs::File;
#[cfg(not(unix))]
use std::io::Read;
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use antiburn_local::discovery::SourceStat;
use anyhow::{Context, Result, ensure};

#[derive(Debug)]
pub(crate) struct BudgetExhausted(&'static str);

impl std::fmt::Display for BudgetExhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for BudgetExhausted {}

/// Directory entries are charged before their kind is inspected.
pub const MAX_DIRECTORY_ENTRIES: usize = 50_000;
/// Invalid and rejected transcript-shaped files also consume this allowance.
pub const MAX_CANDIDATES_INSPECTED: usize = 10_000;
/// One transcript preview stays small enough to avoid loading large histories.
pub const MAX_PREVIEW_BYTES_PER_FILE: u64 = 1024 * 1024;
/// All transcript previews in one request share this memory and I/O allowance.
pub const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;
/// Companion titles use a smaller prefix than session metadata.
pub const MAX_LABEL_BYTES_PER_FILE: u64 = 64 * 1024;
/// Labels share an allowance so a large roster cannot multiply prefix reads.
pub const MAX_LABEL_BYTES: u64 = 4 * 1024 * 1024;
/// A Codex title index is optional enrichment and never merits an unbounded scan.
pub const MAX_TITLE_INDEX_BYTES: u64 = 4 * 1024 * 1024;
/// One helper request yields before it can monopolize an SSH connection.
pub const MAX_REQUEST_ELAPSED: Duration = Duration::from_secs(40);

#[derive(Clone, Copy, Debug)]
pub(crate) struct BudgetLimits {
    pub(crate) directory_entries: usize,
    pub(crate) candidates: usize,
    pub(crate) preview_bytes: u64,
    pub(crate) label_bytes: u64,
    pub(crate) title_index_bytes: u64,
    pub(crate) elapsed: Duration,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            directory_entries: MAX_DIRECTORY_ENTRIES,
            candidates: MAX_CANDIDATES_INSPECTED,
            preview_bytes: MAX_PREVIEW_BYTES,
            label_bytes: MAX_LABEL_BYTES,
            title_index_bytes: MAX_TITLE_INDEX_BYTES,
            elapsed: MAX_REQUEST_ELAPSED,
        }
    }
}

#[derive(Clone, Copy)]
enum ReadClass {
    Preview,
    Label,
    TitleIndex,
}

/// All discovery and companion work for one operation shares this counter.
pub(crate) struct RequestBudget {
    limits: BudgetLimits,
    elapsed: Box<dyn Fn() -> Duration>,
    directory_entries: usize,
    candidates: usize,
    preview_bytes: u64,
    label_bytes: u64,
    title_index_bytes: u64,
}

impl RequestBudget {
    pub(crate) fn new() -> Self {
        Self::with_limits(BudgetLimits::default())
    }

    pub(crate) fn with_limits(limits: BudgetLimits) -> Self {
        let started = Instant::now();
        Self::with_clock(limits, move || started.elapsed())
    }

    fn with_clock(limits: BudgetLimits, elapsed: impl Fn() -> Duration + 'static) -> Self {
        Self {
            limits,
            elapsed: Box::new(elapsed),
            directory_entries: 0,
            candidates: 0,
            preview_bytes: 0,
            label_bytes: 0,
            title_index_bytes: 0,
        }
    }

    pub(crate) fn check_deadline(&self) -> Result<()> {
        if (self.elapsed)() >= self.limits.elapsed {
            return Err(BudgetExhausted("Remote evidence request exceeded its time limit").into());
        }
        Ok(())
    }

    pub(crate) fn visit_entry(&mut self) -> Result<()> {
        self.check_deadline()?;
        if self.directory_entries >= self.limits.directory_entries {
            return Err(
                BudgetExhausted("Remote evidence directory-entry budget was exhausted").into(),
            );
        }
        self.directory_entries += 1;
        Ok(())
    }

    pub(crate) fn inspect_candidate(&mut self) -> Result<()> {
        self.check_deadline()?;
        if self.candidates >= self.limits.candidates {
            return Err(BudgetExhausted("Remote evidence candidate budget was exhausted").into());
        }
        self.candidates += 1;
        Ok(())
    }

    pub(crate) fn read_preview(&mut self, file: &File) -> Result<BoundedRead> {
        self.read(file, ReadClass::Preview, MAX_PREVIEW_BYTES_PER_FILE)
    }

    pub(crate) fn read_origin(&mut self, file: &File) -> Result<BoundedRead> {
        let mut limit = 512;
        loop {
            let mut read = self.read(file, ReadClass::Preview, limit)?;
            if let Some(end) = read.bytes.iter().position(|byte| *byte == b'\n') {
                read.bytes.truncate(end + 1);
                return Ok(read);
            }
            if !read.truncated {
                return Ok(read);
            }
            if limit == MAX_PREVIEW_BYTES_PER_FILE {
                return Err(
                    BudgetExhausted("Remote session origin exceeds the preview limit").into(),
                );
            }
            limit = (limit * 2).min(MAX_PREVIEW_BYTES_PER_FILE);
        }
    }

    pub(crate) fn read_label(&mut self, file: &File) -> Result<BoundedRead> {
        self.read(file, ReadClass::Label, MAX_LABEL_BYTES_PER_FILE)
    }

    pub(crate) fn read_title_index(&mut self, file: &File) -> Result<BoundedRead> {
        self.read(file, ReadClass::TitleIndex, MAX_TITLE_INDEX_BYTES)
    }

    fn read(&mut self, file: &File, class: ReadClass, per_file: u64) -> Result<BoundedRead> {
        self.check_deadline()?;
        let (used, limit) = match class {
            ReadClass::Preview => (&mut self.preview_bytes, self.limits.preview_bytes),
            ReadClass::Label => (&mut self.label_bytes, self.limits.label_bytes),
            ReadClass::TitleIndex => (&mut self.title_index_bytes, self.limits.title_index_bytes),
        };
        let remaining = limit.saturating_sub(*used);
        if remaining == 0 {
            return Err(BudgetExhausted("Remote evidence byte budget was exhausted").into());
        }
        let maximum = remaining.min(per_file);
        let expected = file.metadata()?.len();
        let bytes = read_prefix(file, maximum)?;
        *used = used
            .checked_add(u64::try_from(bytes.len())?)
            .context("Remote evidence byte accounting overflow")?;
        Ok(BoundedRead {
            truncated: expected > u64::try_from(bytes.len())?,
            bytes,
        })
    }

    #[cfg(test)]
    pub(crate) fn candidate_count(&self) -> usize {
        self.candidates
    }

    #[cfg(test)]
    pub(crate) fn label_byte_count(&self) -> u64 {
        self.label_bytes
    }
}

#[cfg(unix)]
fn read_prefix(file: &File, maximum: u64) -> Result<Vec<u8>> {
    use std::os::unix::fs::FileExt;

    let capacity = usize::try_from(maximum.min(1024 * 1024))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut offset = 0u64;
    while offset < maximum {
        let chunk = usize::try_from((maximum - offset).min(64 * 1024))?;
        let start = bytes.len();
        bytes.resize(start + chunk, 0);
        let read = file.read_at(&mut bytes[start..], offset)?;
        bytes.truncate(start + read);
        if read == 0 {
            break;
        }
        offset = offset
            .checked_add(u64::try_from(read)?)
            .context("Remote evidence read offset overflow")?;
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_prefix(file: &File, maximum: u64) -> Result<Vec<u8>> {
    let reader = file.try_clone()?;
    let mut bytes = Vec::with_capacity(usize::try_from(maximum.min(1024 * 1024))?);
    reader.take(maximum).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) struct BoundedRead {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

impl BoundedRead {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// One configured store root. Relative opens never follow a component symlink.
pub(crate) struct TrustedRoot {
    path: PathBuf,
    directory: File,
}

impl TrustedRoot {
    pub(crate) fn open(path: &Path) -> Result<Arc<Self>> {
        let path = path
            .canonicalize()
            .with_context(|| format!("Cannot resolve trusted root: {}", path.display()))?;
        let directory = open_root_directory(&path)?;
        ensure!(
            directory.metadata()?.is_dir(),
            "Trusted root is not a directory"
        );
        Ok(Arc::new(Self { path, directory }))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn admit(self: &Arc<Self>, relative: &Path) -> Result<AdmittedFile> {
        let file = open_relative_file(&self.directory, relative)?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "Evidence is not a regular file");
        let stat = SourceStat::from_open_std_file(&file).context("Cannot stat evidence")?;
        Ok(AdmittedFile {
            root: Arc::clone(self),
            relative: relative.to_path_buf(),
            file,
            stat,
        })
    }
}

pub(crate) struct AdmittedFile {
    pub(crate) root: Arc<TrustedRoot>,
    pub(crate) relative: PathBuf,
    pub(crate) file: File,
    pub(crate) stat: SourceStat,
}

impl AdmittedFile {
    pub(crate) fn display_path(&self) -> PathBuf {
        self.root.path.join(&self.relative)
    }

    pub(crate) fn pinned_size(&self) -> u64 {
        self.stat.size
    }

    pub(crate) fn pinned_version(&self) -> String {
        version_from_stat(&self.stat)
    }

    pub(crate) fn validate_unchanged(&self) -> Result<()> {
        let stat = SourceStat::from_open_std_file(&self.file).context("Cannot stat evidence")?;
        ensure!(
            stat == self.stat,
            "Session changed during collection; retry"
        );
        Ok(())
    }

    pub(crate) fn descriptor_path(&self) -> PathBuf {
        descriptor_path(&self.file)
    }
}

fn version_from_stat(stat: &SourceStat) -> String {
    format!(
        "{}:{}:{}:{}",
        stat.identity.as_deref().unwrap_or("-"),
        stat.size,
        stat.modified_nanos.unwrap_or_default(),
        stat.changed_nanos.unwrap_or_default()
    )
}

#[cfg(unix)]
fn open_root_directory(path: &Path) -> Result<File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())?;
    // O_NONBLOCK makes a mistaken special-file target safe to inspect.
    // SAFETY: path stays valid for the call. The flags omit O_CREAT and need no mode.
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: open returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(not(unix))]
fn open_root_directory(_path: &Path) -> Result<File> {
    anyhow::bail!("Descriptor-relative evidence admission is unsupported on this platform")
}

#[cfg(unix)]
fn open_relative_file(root: &File, relative: &Path) -> Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    ensure!(!relative.as_os_str().is_empty(), "Evidence path is empty");
    let mut components = relative.components().peekable();
    let mut directory = root.try_clone()?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            anyhow::bail!("Evidence path contains an unsupported component")
        };
        let name = CString::new(name.as_bytes())?;
        let is_leaf = components.peek().is_none();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_leaf { 0 } else { libc::O_DIRECTORY };
        // SAFETY: directory stays open and name stays valid for the call.
        // The flags omit O_CREAT and need no mode.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("Evidence path is unavailable: {}", relative.display()));
        }
        // SAFETY: openat returned a new owned descriptor.
        let opened = unsafe { File::from_raw_fd(descriptor) };
        if is_leaf {
            ensure!(
                opened.metadata()?.is_file(),
                "Evidence is not a regular file"
            );
            return Ok(opened);
        }
        ensure!(
            opened.metadata()?.is_dir(),
            "Evidence component is not a directory"
        );
        directory = opened;
    }
    unreachable!()
}

#[cfg(not(unix))]
fn open_relative_file(_root: &File, _relative: &Path) -> Result<File> {
    anyhow::bail!("Descriptor-relative evidence admission is unsupported on this platform")
}

#[cfg(target_os = "linux")]
fn descriptor_path(file: &File) -> PathBuf {
    use std::os::fd::AsRawFd;
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn descriptor_path(file: &File) -> PathBuf {
    use std::os::fd::AsRawFd;
    PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))
}

#[cfg(not(unix))]
fn descriptor_path(_file: &File) -> PathBuf {
    PathBuf::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn admitted_descriptor_survives_path_replacement_without_reading_replacement() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("session.jsonl"), b"original").unwrap();
        let root = TrustedRoot::open(directory.path()).unwrap();
        let admitted = root.admit(Path::new("session.jsonl")).unwrap();

        std::fs::rename(
            directory.path().join("session.jsonl"),
            directory.path().join("old.jsonl"),
        )
        .unwrap();
        std::fs::write(directory.path().join("session.jsonl"), b"outside-marker").unwrap();

        let mut content = String::new();
        admitted
            .file
            .try_clone()
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "original");
    }

    #[cfg(unix)]
    #[test]
    fn relative_open_rejects_leaf_and_intermediate_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("marker.jsonl"), b"outside-marker").unwrap();
        symlink(
            outside.path().join("marker.jsonl"),
            directory.path().join("leaf.jsonl"),
        )
        .unwrap();
        symlink(outside.path(), directory.path().join("nested")).unwrap();
        let root = TrustedRoot::open(directory.path()).unwrap();
        assert!(root.admit(Path::new("leaf.jsonl")).is_err());
        assert!(root.admit(Path::new("nested/marker.jsonl")).is_err());
    }

    #[test]
    fn request_budget_enforces_exact_entry_and_byte_boundaries() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"12345").unwrap();
        let limits = BudgetLimits {
            directory_entries: 1,
            candidates: 1,
            preview_bytes: 5,
            label_bytes: 5,
            title_index_bytes: 5,
            elapsed: Duration::from_secs(1),
        };
        let mut budget = RequestBudget::with_limits(limits);
        budget.visit_entry().unwrap();
        assert!(budget.visit_entry().is_err());
        budget.inspect_candidate().unwrap();
        assert!(budget.inspect_candidate().is_err());
        assert_eq!(budget.read_preview(&file).unwrap().bytes, b"12345");
        assert!(budget.read_preview(&file).is_err());
    }

    #[test]
    fn admitted_size_and_version_stay_pinned_and_detect_growth() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, b"original").unwrap();
        let root = TrustedRoot::open(directory.path()).unwrap();
        let admitted = root.admit(Path::new("session.jsonl")).unwrap();
        let pinned = admitted.pinned_version();

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"-growth")
            .unwrap();

        assert_eq!(admitted.pinned_version(), pinned);
        assert_eq!(admitted.pinned_size(), 8);
        assert!(admitted.validate_unchanged().is_err());
    }

    #[test]
    fn injected_clock_stops_work_at_the_next_budget_check() {
        let elapsed_nanos = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&elapsed_nanos);
        let mut budget = RequestBudget::with_clock(
            BudgetLimits {
                elapsed: Duration::from_nanos(5),
                ..BudgetLimits::default()
            },
            move || Duration::from_nanos(observed.load(Ordering::SeqCst)),
        );
        budget.visit_entry().unwrap();
        elapsed_nanos.store(5, Ordering::SeqCst);
        assert!(budget.inspect_candidate().is_err());
        assert_eq!(budget.candidate_count(), 0);
    }
}
