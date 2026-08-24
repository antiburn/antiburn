// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded newline framing for JSONL transcript records.
//!
//! The reader retains at most [`MAX_RECORD_BYTES`] for one record. It drains an
//! oversized record through its newline before it reads the next record.

use std::io::{self, BufRead};

pub const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
pub const SCAN_QUANTUM_BYTES: usize = 64 * 1024;

pub struct BoundedJsonlReader<R: BufRead> {
    source: R,
    record: Vec<u8>,
    max_record_bytes: usize,
    retained_record_bytes_high_water: usize,
    next_index: u64,
    terminal: bool,
}

impl<R: BufRead> BoundedJsonlReader<R> {
    pub fn new(source: R) -> Self {
        Self::with_max_record_bytes(source, MAX_RECORD_BYTES)
    }

    pub fn with_max_record_bytes(source: R, max_record_bytes: usize) -> Self {
        Self {
            source,
            record: Vec::new(),
            max_record_bytes,
            retained_record_bytes_high_water: 0,
            next_index: 0,
            terminal: false,
        }
    }

    pub fn next_record(&mut self, cancel: &dyn Fn() -> bool) -> Option<FramedRecord<'_>> {
        if self.terminal {
            return None;
        }

        loop {
            self.record.clear();
            let mut record_bytes = 0_u64;
            let mut oversized = false;

            if cancel() {
                let index = self.take_index();
                return Some(self.finish_terminal(RecordSkip::Cancelled { index }));
            }

            loop {
                let available = match self.source.fill_buf() {
                    Ok(available) => available,
                    Err(error) => {
                        let kind = error.kind();
                        let index = self.take_index();
                        return Some(self.finish_terminal(RecordSkip::ReadFailed { index, kind }));
                    }
                };

                if available.is_empty() {
                    if record_bytes == 0 {
                        self.terminal = true;
                        return None;
                    }

                    let index = self.take_index();
                    return Some(self.finish_terminal(RecordSkip::IncompleteTail {
                        index,
                        dropped_bytes: record_bytes,
                    }));
                }

                let step_len = available.len().min(SCAN_QUANTUM_BYTES);
                if cancel() {
                    let index = self.take_index();
                    return Some(self.finish_terminal(RecordSkip::Cancelled { index }));
                }

                let step = &available[..step_len];
                let newline = step.iter().position(|byte| *byte == b'\n');
                let content_len = newline.unwrap_or(step_len);
                record_bytes = record_bytes.saturating_add(content_len as u64);

                if !oversized {
                    let remaining = self.max_record_bytes - self.record.len();
                    let retained = content_len.min(remaining);
                    if retained > 0 {
                        self.record.reserve_exact(retained);
                        self.record.extend_from_slice(&step[..retained]);
                        self.retained_record_bytes_high_water =
                            self.retained_record_bytes_high_water.max(self.record.len());
                    }
                    if content_len > remaining {
                        oversized = true;
                        self.record.clear();
                    }
                }

                let consumed = newline.map_or(step_len, |position| position + 1);
                self.source.consume(consumed);

                if newline.is_none() {
                    continue;
                }

                let index = self.take_index();
                if oversized {
                    return Some(FramedRecord::Skipped(RecordSkip::Oversized {
                        index,
                        dropped_bytes: record_bytes,
                    }));
                }

                if self.record.last() == Some(&b'\r') {
                    self.record.pop();
                }
                if is_whitespace_only(&self.record) {
                    self.next_index = index;
                    break;
                }

                return Some(FramedRecord::Complete {
                    index,
                    bytes: &self.record,
                });
            }
        }
    }

    pub fn retained_record_bytes_high_water(&self) -> usize {
        self.retained_record_bytes_high_water
    }

    fn take_index(&mut self) -> u64 {
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        index
    }

    fn finish_terminal(&mut self, skip: RecordSkip) -> FramedRecord<'_> {
        self.record.clear();
        self.terminal = true;
        FramedRecord::Skipped(skip)
    }
}

fn is_whitespace_only(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|record| record.trim().is_empty())
}

pub enum FramedRecord<'a> {
    Complete { index: u64, bytes: &'a [u8] },
    Skipped(RecordSkip),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSkip {
    Oversized { index: u64, dropped_bytes: u64 },
    IncompleteTail { index: u64, dropped_bytes: u64 },
    Cancelled { index: u64 },
    ReadFailed { index: u64, kind: io::ErrorKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialReason {
    Oversized,
    MalformedRecord,
    IncompleteTail,
    Cancelled,
    ReadFailed,
}

impl RecordSkip {
    pub fn partial_reason(&self) -> PartialReason {
        match self {
            Self::Oversized { .. } => PartialReason::Oversized,
            Self::IncompleteTail { .. } => PartialReason::IncompleteTail,
            Self::Cancelled { .. } => PartialReason::Cancelled,
            Self::ReadFailed { .. } => PartialReason::ReadFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::{BufReader, Cursor, Error, Read};
    use std::rc::Rc;

    const NEVER_CANCEL: &dyn Fn() -> bool = &|| false;

    #[test]
    fn frames_records_and_excludes_the_newline() {
        let mut reader = BoundedJsonlReader::new(Cursor::new(b"one\ntwo\nthree\n"));

        assert_complete(reader.next_record(NEVER_CANCEL), 0, b"one");
        assert_complete(reader.next_record(NEVER_CANCEL), 1, b"two");
        assert_complete(reader.next_record(NEVER_CANCEL), 2, b"three");
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn trailing_newline_is_not_an_incomplete_tail() {
        let mut reader = BoundedJsonlReader::new(Cursor::new(b"one\n"));

        assert_complete(reader.next_record(NEVER_CANCEL), 0, b"one");
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn bytes_without_a_newline_are_an_incomplete_tail() {
        let mut reader = BoundedJsonlReader::new(Cursor::new(b"unfinished"));

        assert_eq!(
            skip(reader.next_record(NEVER_CANCEL)),
            RecordSkip::IncompleteTail {
                index: 0,
                dropped_bytes: 10,
            }
        );
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn record_exactly_at_the_limit_is_complete() {
        let mut reader = BoundedJsonlReader::with_max_record_bytes(Cursor::new(b"abcd\n"), 4);

        assert_complete(reader.next_record(NEVER_CANCEL), 0, b"abcd");
        assert_eq!(reader.retained_record_bytes_high_water(), 4);
    }

    #[test]
    fn record_one_byte_over_the_limit_is_oversized() {
        let mut reader = BoundedJsonlReader::with_max_record_bytes(Cursor::new(b"abcde\n"), 4);

        assert_eq!(
            skip(reader.next_record(NEVER_CANCEL)),
            RecordSkip::Oversized {
                index: 0,
                dropped_bytes: 5,
            }
        );
        assert_eq!(reader.retained_record_bytes_high_water(), 4);
    }

    #[test]
    fn oversized_record_is_drained_and_the_next_record_is_read() {
        let mut reader =
            BoundedJsonlReader::with_max_record_bytes(Cursor::new(b"one\ntoo-large\nthree\n"), 5);

        assert_complete(reader.next_record(NEVER_CANCEL), 0, b"one");
        let diagnostic = reader.next_record(NEVER_CANCEL);
        assert_eq!(
            skip(diagnostic),
            RecordSkip::Oversized {
                index: 1,
                dropped_bytes: 9,
            }
        );
        assert_complete(reader.next_record(NEVER_CANCEL), 2, b"three");
    }

    #[test]
    fn retained_record_bytes_never_exceed_the_bound_on_a_large_source() {
        let source_bytes = MAX_RECORD_BYTES + (3 * SCAN_QUANTUM_BYTES);
        let bytes_read = Rc::new(Cell::new(0));
        let source =
            RepeatedByteReader::new(source_bytes, SCAN_QUANTUM_BYTES / 2, bytes_read.clone());
        let mut reader =
            BoundedJsonlReader::new(BufReader::with_capacity(SCAN_QUANTUM_BYTES * 2, source));

        assert_eq!(
            skip(reader.next_record(NEVER_CANCEL)),
            RecordSkip::Oversized {
                index: 0,
                dropped_bytes: source_bytes as u64,
            }
        );
        assert_eq!(bytes_read.get(), source_bytes + 1);
        assert!(reader.retained_record_bytes_high_water() <= MAX_RECORD_BYTES);
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn whitespace_only_lines_are_skipped_without_a_diagnostic() {
        let mut reader = BoundedJsonlReader::new(Cursor::new(b"\n   \n\t\t\n\r\nvalue\n"));

        assert_complete(reader.next_record(NEVER_CANCEL), 0, b"value");
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn partial_reason_maps_every_record_skip_variant() {
        let variants = [
            (
                RecordSkip::Oversized {
                    index: 0,
                    dropped_bytes: 1,
                },
                PartialReason::Oversized,
            ),
            (
                RecordSkip::IncompleteTail {
                    index: 1,
                    dropped_bytes: 2,
                },
                PartialReason::IncompleteTail,
            ),
            (RecordSkip::Cancelled { index: 2 }, PartialReason::Cancelled),
            (
                RecordSkip::ReadFailed {
                    index: 3,
                    kind: io::ErrorKind::InvalidData,
                },
                PartialReason::ReadFailed,
            ),
        ];

        for (skip, expected) in variants {
            assert_eq!(skip.partial_reason(), expected);
        }
    }

    #[test]
    fn diagnostics_carry_no_transcript_content() {
        const MARKER: &str = "PRIVATE_TRANSCRIPT_MARKER";
        let mut oversized = BoundedJsonlReader::with_max_record_bytes(
            Cursor::new(format!("{MARKER}\n").into_bytes()),
            1,
        );
        let oversized = skip(oversized.next_record(NEVER_CANCEL));

        let mut incomplete = BoundedJsonlReader::new(Cursor::new(MARKER.as_bytes()));
        let incomplete = skip(incomplete.next_record(NEVER_CANCEL));

        let mut cancelled = BoundedJsonlReader::new(Cursor::new(MARKER.as_bytes()));
        let cancelled = skip(cancelled.next_record(&|| true));

        let source = DataThenError::new(MARKER.as_bytes());
        let mut failed = BoundedJsonlReader::new(BufReader::new(source));
        let failed = skip(failed.next_record(NEVER_CANCEL));

        for diagnostic in [oversized, incomplete, cancelled, failed] {
            assert!(!format!("{diagnostic:?}").contains(MARKER));
        }
    }

    #[test]
    fn cancellation_stops_between_records() {
        let mut reader = BoundedJsonlReader::new(Cursor::new(b"one\ntwo\n"));

        assert_complete(reader.next_record(NEVER_CANCEL), 0, b"one");
        assert_eq!(
            skip(reader.next_record(&|| true)),
            RecordSkip::Cancelled { index: 1 }
        );
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn cancellation_stops_while_draining_an_oversized_record() {
        let mut source = vec![b'x'; SCAN_QUANTUM_BYTES * 3];
        source.push(b'\n');
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == 4
        };
        let mut reader = BoundedJsonlReader::with_max_record_bytes(Cursor::new(source), 1);

        assert_eq!(
            skip(reader.next_record(&cancel)),
            RecordSkip::Cancelled { index: 0 }
        );
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn cancellation_is_checked_inside_a_buffer_larger_than_the_scan_quantum() {
        let source = vec![b'x'; SCAN_QUANTUM_BYTES * 3];
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        let mut reader = BoundedJsonlReader::new(Cursor::new(source));

        assert_eq!(
            skip(reader.next_record(&cancel)),
            RecordSkip::Cancelled { index: 0 }
        );
        assert_eq!(reader.source.position(), SCAN_QUANTUM_BYTES as u64);
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    #[test]
    fn read_failure_is_terminal() {
        let mut reader = BoundedJsonlReader::new(BufReader::new(DataThenError::new(b"partial")));

        assert_eq!(
            skip(reader.next_record(NEVER_CANCEL)),
            RecordSkip::ReadFailed {
                index: 0,
                kind: io::ErrorKind::Other,
            }
        );
        assert!(reader.next_record(NEVER_CANCEL).is_none());
    }

    fn assert_complete(outcome: Option<FramedRecord<'_>>, expected_index: u64, expected: &[u8]) {
        match outcome {
            Some(FramedRecord::Complete { index, bytes }) => {
                assert_eq!(index, expected_index);
                assert_eq!(bytes, expected);
            }
            Some(FramedRecord::Skipped(skip)) => panic!("expected a complete record, got {skip:?}"),
            None => panic!("expected a complete record, got end of input"),
        }
    }

    fn skip(outcome: Option<FramedRecord<'_>>) -> RecordSkip {
        match outcome {
            Some(FramedRecord::Skipped(skip)) => skip,
            Some(FramedRecord::Complete { .. }) => panic!("expected a skipped record"),
            None => panic!("expected a skipped record, got end of input"),
        }
    }

    struct RepeatedByteReader {
        remaining: usize,
        max_chunk: usize,
        newline_pending: bool,
        bytes_read: Rc<Cell<usize>>,
    }

    impl RepeatedByteReader {
        fn new(remaining: usize, max_chunk: usize, bytes_read: Rc<Cell<usize>>) -> Self {
            Self {
                remaining,
                max_chunk,
                newline_pending: true,
                bytes_read,
            }
        }
    }

    impl Read for RepeatedByteReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.remaining > 0 {
                let count = output.len().min(self.max_chunk).min(self.remaining);
                output[..count].fill(b'x');
                self.remaining -= count;
                self.bytes_read.set(self.bytes_read.get() + count);
                return Ok(count);
            }

            if self.newline_pending && !output.is_empty() {
                output[0] = b'\n';
                self.newline_pending = false;
                self.bytes_read.set(self.bytes_read.get() + 1);
                return Ok(1);
            }

            Ok(0)
        }
    }

    struct DataThenError {
        data: Vec<u8>,
        returned_data: bool,
    }

    impl DataThenError {
        fn new(data: &[u8]) -> Self {
            Self {
                data: data.to_vec(),
                returned_data: false,
            }
        }
    }

    impl Read for DataThenError {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if !self.returned_data {
                let count = output.len().min(self.data.len());
                output[..count].copy_from_slice(&self.data[..count]);
                self.returned_data = true;
                return Ok(count);
            }

            Err(Error::other("synthetic read failure"))
        }
    }
}
