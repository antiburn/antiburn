use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use antiburn_local::analysis::{
    AppendOnlyGuarantee, ClaudeAdapter, NormalizedRecord, RawSource, RecordSink, SessionCollector,
    SessionInput, SessionSummary, SourceChangedReason, SourceClaim, VisitOutcome,
    append_only_guarantee,
};
use antiburn_local::discovery::source_version::{
    FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of,
};
use tempfile::TempDir;

fn record(text: &str, minute: usize) -> String {
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":\"2024-06-01T12:{minute:02}:00Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n"
    )
}

fn write_source(directory: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = directory.path().join(name);
    std::fs::write(&path, bytes).expect("write source");
    path
}

fn file_input(path: &Path) -> SessionInput {
    SessionInput {
        agent: "claude".to_string(),
        session_id: "claimed-session".to_string(),
        source: RawSource::File(path.to_path_buf()),
    }
}

fn claim_for_path(path: &Path) -> SourceClaim {
    let mut file = File::open(path).expect("open source for claim");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).expect("read source for claim");
    SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&bytes)),
    })
}

#[derive(Default)]
struct CountingSink {
    records: usize,
    finishes: usize,
}

impl RecordSink for CountingSink {
    fn record(&mut self, _record: NormalizedRecord) {
        self.records += 1;
    }

    fn finish(&mut self, _summary: SessionSummary) {
        self.finishes += 1;
    }
}

#[test]
fn a_replacement_before_pinning_streams_nothing() {
    let directory = TempDir::new().expect("tempdir");
    let bytes = record("same bytes", 0);
    let path = write_source(&directory, "session.jsonl", bytes.as_bytes());
    let replacement = write_source(&directory, "replacement.jsonl", bytes.as_bytes());
    let claim = claim_for_path(&path);
    std::fs::rename(replacement, &path).expect("replace source path");
    let mut sink = CountingSink::default();

    let outcome = ClaudeAdapter
        .visit_claimed(
            &file_input(&path),
            &claim,
            AppendOnlyGuarantee::Evidenced,
            &|| false,
            &mut sink,
        )
        .expect("visit replacement");

    assert_eq!(
        outcome,
        VisitOutcome::SourceChanged(SourceChangedReason::IdentityMismatch)
    );
    assert_eq!(sink.finishes, 0);
    assert_eq!(sink.records, 0);
}

#[cfg(unix)]
#[test]
fn a_rename_after_pinning_is_accepted_on_the_original_inode() {
    let directory = TempDir::new().expect("tempdir");
    let source = [record("first", 0), record("second", 1)].concat();
    let path = write_source(&directory, "session.jsonl", source.as_bytes());
    let replacement = write_source(&directory, "replacement.jsonl", b"");
    let claim = claim_for_path(&path);
    let mut sink = RenameSink::new(&path, replacement);

    let outcome = ClaudeAdapter
        .visit_claimed(
            &file_input(&path),
            &claim,
            AppendOnlyGuarantee::Evidenced,
            &|| false,
            &mut sink,
        )
        .expect("visit renamed source");

    assert_eq!(
        outcome,
        VisitOutcome::AcceptedPrefix {
            boundary: claim.boundary,
        }
    );
    assert_eq!(
        sink.collector
            .into_session()
            .expect("original inode must publish")
            .events
            .len(),
        2
    );
}

#[test]
fn a_rewritten_record_after_the_head_region_passes_the_recheck() {
    let directory = TempDir::new().expect("tempdir");
    let (source, rewrite_offset) = large_source();
    let path = write_source(&directory, "session.jsonl", &source);
    let claim = claim_for_path(&path);
    let mut sink = RewriteSink::new(&path, rewrite_offset);

    let outcome = ClaudeAdapter
        .visit_claimed(
            &file_input(&path),
            &claim,
            AppendOnlyGuarantee::Evidenced,
            &|| false,
            &mut sink,
        )
        .expect("visit rewritten prefix");

    // The prefix recheck covers only the head region.
    // The append-only guarantee must protect later complete records.
    assert_eq!(
        outcome,
        VisitOutcome::AcceptedPrefix {
            boundary: claim.boundary,
        }
    );
}

#[test]
fn two_reads_of_the_same_identity_and_boundary_are_byte_identical() {
    let directory = TempDir::new().expect("tempdir");
    let path = write_source(&directory, "session.jsonl", record("first", 0).as_bytes());
    let claim = claim_for_path(&path);
    let input = file_input(&path);
    let mut first = SessionCollector::new("claude", "claimed-session");
    let first_outcome = ClaudeAdapter
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Evidenced,
            &|| false,
            &mut first,
        )
        .expect("visit first prefix");
    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open source for append")
        .write_all(record("second", 1).as_bytes())
        .expect("append source");
    let mut second = SessionCollector::new("claude", "claimed-session");
    let second_outcome = ClaudeAdapter
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Evidenced,
            &|| false,
            &mut second,
        )
        .expect("visit second prefix");

    assert_eq!(first_outcome, second_outcome);
    assert_eq!(
        serde_json::to_vec(&first.into_session().expect("first prefix must publish"))
            .expect("serialize first prefix"),
        serde_json::to_vec(&second.into_session().expect("second prefix must publish"))
            .expect("serialize second prefix")
    );
}

#[test]
fn claude_carries_no_append_only_guarantee() {
    assert_eq!(append_only_guarantee("claude"), AppendOnlyGuarantee::Absent);
}

#[test]
fn an_absent_guarantee_accepts_a_stable_full_read() {
    let directory = TempDir::new().expect("tempdir");
    let path = write_source(&directory, "session.jsonl", record("stable", 0).as_bytes());
    let claim = claim_for_path(&path);
    let mut collector = SessionCollector::new("claude", "claimed-session");

    let outcome = ClaudeAdapter
        .visit_claimed(
            &file_input(&path),
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut collector,
        )
        .expect("visit stable full source");

    assert_eq!(outcome, VisitOutcome::AcceptedFull);
    assert!(collector.into_session().is_ok());
}

#[test]
fn an_absent_guarantee_takes_the_full_reprocess_path() {
    let directory = TempDir::new().expect("tempdir");
    let path = write_source(&directory, "session.jsonl", record("first", 0).as_bytes());
    let claim = claim_for_path(&path);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open source for append")
        .write_all(record("second", 1).as_bytes())
        .expect("append source");
    let mut collector = SessionCollector::new("claude", "claimed-session");

    let outcome = ClaudeAdapter
        .visit_claimed(
            &file_input(&path),
            &claim,
            append_only_guarantee("claude"),
            &|| false,
            &mut collector,
        )
        .expect("visit full source");

    assert_eq!(
        outcome,
        VisitOutcome::SourceChanged(SourceChangedReason::FingerprintMismatch)
    );
    assert!(collector.into_session().is_err());
}

#[test]
fn an_evidenced_guarantee_reads_a_pinned_prefix() {
    let directory = TempDir::new().expect("tempdir");
    let path = write_source(&directory, "session.jsonl", record("first", 0).as_bytes());
    let claim = claim_for_path(&path);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open source for append")
        .write_all(record("second", 1).as_bytes())
        .expect("append source");
    let mut collector = SessionCollector::new("claude", "claimed-session");

    let outcome = ClaudeAdapter
        .visit_claimed(
            &file_input(&path),
            &claim,
            AppendOnlyGuarantee::Evidenced,
            &|| false,
            &mut collector,
        )
        .expect("visit prefix");

    assert_eq!(
        outcome,
        VisitOutcome::AcceptedPrefix {
            boundary: claim.boundary,
        }
    );
    assert_eq!(
        collector
            .into_session()
            .expect("evidenced prefix must publish")
            .events
            .len(),
        1
    );
}

#[cfg(unix)]
struct RenameSink {
    collector: SessionCollector,
    path: PathBuf,
    replacement: PathBuf,
    renamed: bool,
}

#[cfg(unix)]
impl RenameSink {
    fn new(path: &Path, replacement: PathBuf) -> Self {
        Self {
            collector: SessionCollector::new("claude", "claimed-session"),
            path: path.to_path_buf(),
            replacement,
            renamed: false,
        }
    }
}

#[cfg(unix)]
impl RecordSink for RenameSink {
    fn record(&mut self, record: NormalizedRecord) {
        if !self.renamed {
            std::fs::rename(&self.replacement, &self.path).expect("replace pinned path");
            self.renamed = true;
        }
        self.collector.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.collector.finish(summary);
    }
}

struct RewriteSink {
    collector: SessionCollector,
    path: PathBuf,
    rewrite_offset: u64,
    rewritten: bool,
}

impl RewriteSink {
    fn new(path: &Path, rewrite_offset: u64) -> Self {
        Self {
            collector: SessionCollector::new("claude", "claimed-session"),
            path: path.to_path_buf(),
            rewrite_offset,
            rewritten: false,
        }
    }
}

impl RecordSink for RewriteSink {
    fn record(&mut self, record: NormalizedRecord) {
        if !self.rewritten {
            let mut file = OpenOptions::new()
                .write(true)
                .open(&self.path)
                .expect("open source for rewrite");
            file.seek(SeekFrom::Start(self.rewrite_offset))
                .expect("seek rewritten record");
            file.write_all(b"bbbb").expect("rewrite complete record");
            file.sync_all().expect("sync complete record");
            self.rewritten = true;
        }
        self.collector.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.collector.finish(summary);
    }
}

fn large_source() -> (Vec<u8>, u64) {
    let mut source = Vec::new();
    let mut minute = 0;
    while source.len() <= FINGERPRINT_HEAD_BYTES + 1024 {
        source.extend_from_slice(
            record(
                &format!("padding-{minute}-{}", "x".repeat(900)),
                minute % 60,
            )
            .as_bytes(),
        );
        minute += 1;
    }
    let target = record("aaaa", minute % 60);
    let marker = target.find("aaaa").expect("target marker") as u64;
    let rewrite_offset = source.len() as u64 + marker;
    source.extend_from_slice(target.as_bytes());
    source.extend_from_slice(record("tail", (minute + 1) % 60).as_bytes());
    (source, rewrite_offset)
}
