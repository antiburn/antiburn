//! Source validation for one file handle.
//!
//! The head hash covers only the configured head region. It does not prove that the full prefix is append-only.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::analysis::interface::SourceChangedReason;
use crate::discovery::source_version::{
    FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceClaim {
    pub fingerprint: String,
    pub identity: Option<String>,
    pub boundary: u64,
    pub head_hash: Option<u64>,
}

impl SourceClaim {
    pub fn from_fingerprint_inputs(inputs: &FingerprintInputs) -> Self {
        Self {
            fingerprint: inputs.fingerprint(),
            identity: inputs.stat.identity.clone(),
            boundary: inputs.stat.size,
            head_hash: inputs.head_hash,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOnlyGuarantee {
    Evidenced,
    Absent,
}

/// Repository fixtures must be synthetic under `CONTRIBUTING.md`.
/// They cannot prove how a third-party writer updates files.
/// One local observation cannot establish a repeatable guarantee.
pub fn append_only_guarantee(_agent: &str) -> AppendOnlyGuarantee {
    AppendOnlyGuarantee::Absent
}

pub type PinnedOpen = Result<PinnedSource, SourceChangedReason>;

pub struct PinnedSource {
    file: File,
    claim: SourceClaim,
    consumed: u64,
}

impl PinnedSource {
    /// Opens the path once and validates the handle before records can stream.
    pub fn open(path: &Path, claim: SourceClaim) -> anyhow::Result<PinnedOpen> {
        let mut file = File::open(path)?;
        let stat = stat_from_handle(&file)?;
        if stat.identity != claim.identity {
            return Ok(Err(SourceChangedReason::IdentityMismatch));
        }
        if stat.size < claim.boundary {
            return Ok(Err(SourceChangedReason::ShortAtOpen {
                size: stat.size,
                boundary: claim.boundary,
            }));
        }
        let head_hash = read_head_hash(&mut file, claim.boundary)?;
        if claim.head_hash.is_some() && claim.head_hash != Some(head_hash) {
            return Ok(Err(SourceChangedReason::HeadRegionMismatch));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(Ok(Self {
            file,
            claim,
            consumed: 0,
        }))
    }

    pub fn claim(&self) -> &SourceClaim {
        &self.claim
    }

    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Reads from offset zero and delivers at most `limit` bytes.
    pub fn reader(&mut self, limit: u64) -> PinnedReader<'_> {
        self.consumed = 0;
        let seek_error = self.file.seek(SeekFrom::Start(0)).err();
        PinnedReader {
            file: &mut self.file,
            consumed: &mut self.consumed,
            remaining: limit,
            seek_error,
        }
    }

    pub fn recheck_prefix(&mut self) -> anyhow::Result<Option<SourceChangedReason>> {
        if self.consumed < self.claim.boundary {
            return Ok(Some(SourceChangedReason::ShortRead {
                consumed: self.consumed,
                boundary: self.claim.boundary,
            }));
        }
        let stat = stat_from_handle(&self.file)?;
        if stat.size < self.claim.boundary {
            return Ok(Some(SourceChangedReason::TruncatedAfterRead {
                size: stat.size,
                boundary: self.claim.boundary,
            }));
        }
        let head_hash = read_head_hash(&mut self.file, self.claim.boundary)?;
        if self.claim.head_hash.is_some() && self.claim.head_hash != Some(head_hash) {
            return Ok(Some(SourceChangedReason::HeadRegionMismatch));
        }
        Ok(None)
    }

    pub fn recheck_full(&mut self) -> anyhow::Result<Option<SourceChangedReason>> {
        let stat = stat_from_handle(&self.file)?;
        let size = stat.size;
        let head_hash = read_head_hash(&mut self.file, size)?;
        let fingerprint = FingerprintInputs {
            stat,
            head_hash: Some(head_hash),
        }
        .fingerprint();
        if fingerprint != self.claim.fingerprint {
            return Ok(Some(SourceChangedReason::FingerprintMismatch));
        }
        Ok(None)
    }
}

pub struct PinnedReader<'a> {
    file: &'a mut File,
    consumed: &'a mut u64,
    remaining: u64,
    seek_error: Option<io::Error>,
}

impl Read for PinnedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(error) = self.seek_error.take() {
            return Err(error);
        }
        if self.remaining == 0 || buffer.is_empty() {
            return Ok(0);
        }
        let available = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        let read_limit = buffer.len().min(available);
        let read = self.file.read(&mut buffer[..read_limit])?;
        self.remaining -= read as u64;
        *self.consumed += read as u64;
        Ok(read)
    }
}

fn stat_from_handle(file: &File) -> anyhow::Result<SourceStat> {
    SourceStat::from_open_std_file(file)
        .ok_or_else(|| anyhow::anyhow!("cannot read source metadata from the open handle"))
}

fn read_head_hash(file: &mut File, boundary: u64) -> io::Result<u64> {
    file.seek(SeekFrom::Start(0))?;
    let expected = usize::try_from(boundary.min(FINGERPRINT_HEAD_BYTES as u64))
        .expect("the fingerprint head limit fits usize");
    let mut bytes = Vec::with_capacity(expected);
    let mut buffer = [0_u8; 8192];
    while bytes.len() < expected {
        let remaining = expected - bytes.len();
        let read_limit = buffer.len().min(remaining);
        let read = file.read(&mut buffer[..read_limit])?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(head_hash_of(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    fn write_source(directory: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).expect("write source");
        path
    }

    fn claim_for_path(path: &Path) -> SourceClaim {
        let mut file = File::open(path).expect("open source for claim");
        let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
        let head_hash = read_head_hash(&mut file, stat.size).expect("hash source for claim");
        SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
            stat,
            head_hash: Some(head_hash),
        })
    }

    #[test]
    fn a_matching_handle_opens_pinned() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"matching source\n");
        let claim = claim_for_path(&path);

        let pinned = PinnedSource::open(&path, claim.clone()).expect("open pinned source");

        assert!(pinned.is_ok());
        assert_eq!(pinned.expect("matching source").claim(), &claim);
    }

    #[test]
    fn a_replaced_path_is_rejected_before_any_record() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"byte-identical source\n");
        let replacement = write_source(&directory, "replacement.jsonl", b"byte-identical source\n");
        let claim = claim_for_path(&path);
        std::fs::rename(replacement, &path).expect("replace source path");

        let result = PinnedSource::open(&path, claim).expect("validate replacement");

        assert!(matches!(result, Err(SourceChangedReason::IdentityMismatch)));
    }

    #[test]
    fn a_source_below_the_boundary_is_rejected_at_open() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"complete source\n");
        let claim = claim_for_path(&path);
        std::fs::write(&path, b"short").expect("shorten source");

        let result = PinnedSource::open(&path, claim.clone()).expect("validate short source");

        assert!(matches!(
            result,
            Err(SourceChangedReason::ShortAtOpen { size: 5, boundary })
                if boundary == claim.boundary
        ));
    }

    #[test]
    fn a_rewritten_head_region_is_rejected_at_open() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"original head\n");
        let claim = claim_for_path(&path);
        std::fs::write(&path, b"rewrittenhead\n").expect("rewrite source head");

        let result = PinnedSource::open(&path, claim).expect("validate rewritten source");

        assert!(matches!(
            result,
            Err(SourceChangedReason::HeadRegionMismatch)
        ));
    }

    #[test]
    fn a_short_read_is_reported_from_the_consumed_count() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"12345678");
        let claim = claim_for_path(&path);
        let mut pinned = PinnedSource::open(&path, claim.clone())
            .expect("open pinned source")
            .expect("matching source");
        OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open source for truncation")
            .set_len(3)
            .expect("truncate source");
        let mut bytes = Vec::new();
        pinned
            .reader(claim.boundary)
            .read_to_end(&mut bytes)
            .expect("read shortened source");

        let result = pinned.recheck_prefix().expect("recheck prefix");

        assert_eq!(bytes, b"123");
        assert_eq!(
            result,
            Some(SourceChangedReason::ShortRead {
                consumed: 3,
                boundary: claim.boundary,
            })
        );
    }

    #[test]
    fn a_truncation_after_the_read_is_reported_from_the_pinned_size() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"12345678");
        let claim = claim_for_path(&path);
        let mut pinned = PinnedSource::open(&path, claim.clone())
            .expect("open pinned source")
            .expect("matching source");
        let mut bytes = Vec::new();
        pinned
            .reader(claim.boundary)
            .read_to_end(&mut bytes)
            .expect("read source");
        OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open source for truncation")
            .set_len(3)
            .expect("truncate source");

        let result = pinned.recheck_prefix().expect("recheck prefix");

        assert_eq!(bytes, b"12345678");
        assert_eq!(
            result,
            Some(SourceChangedReason::TruncatedAfterRead {
                size: 3,
                boundary: claim.boundary,
            })
        );
    }

    #[test]
    fn an_append_past_the_boundary_passes_the_prefix_recheck() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, "session.jsonl", b"first record\n");
        let claim = claim_for_path(&path);
        let mut pinned = PinnedSource::open(&path, claim.clone())
            .expect("open pinned source")
            .expect("matching source");
        let mut bytes = Vec::new();
        pinned
            .reader(claim.boundary)
            .read_to_end(&mut bytes)
            .expect("read prefix");
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open source for append")
            .write_all(b"second record\n")
            .expect("append source");

        assert_eq!(pinned.recheck_prefix().expect("recheck prefix"), None);
        assert_eq!(bytes, b"first record\n");
    }

    #[test]
    fn a_same_size_rewrite_after_the_head_region_fails_the_full_recheck() {
        let directory = TempDir::new().expect("tempdir");
        let bytes = vec![b'a'; FINGERPRINT_HEAD_BYTES + 1024];
        let path = write_source(&directory, "session.jsonl", &bytes);
        let claim = claim_for_path(&path);
        let mut pinned = PinnedSource::open(&path, claim)
            .expect("open pinned source")
            .expect("matching source");
        let mut read = Vec::new();
        pinned
            .reader(u64::MAX)
            .read_to_end(&mut read)
            .expect("read full source");
        thread::sleep(Duration::from_millis(10));
        let mut writer = OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open source for rewrite");
        writer
            .seek(SeekFrom::Start(FINGERPRINT_HEAD_BYTES as u64 + 100))
            .expect("seek after head region");
        writer.write_all(b"b").expect("rewrite source");
        writer.sync_all().expect("sync rewrite");

        assert_eq!(
            pinned.recheck_full().expect("recheck full source"),
            Some(SourceChangedReason::FingerprintMismatch)
        );
    }
}
