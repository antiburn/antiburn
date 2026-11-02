//! Persistent framing for finite reads of a growing JSONL file.

use serde::{Deserialize, Serialize};

use super::MAX_RECORD_BYTES;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LiveJsonlFramer {
    pending: Vec<u8>,
    oversized: bool,
    discard_prefix: bool,
    record_start: u64,
}

impl LiveJsonlFramer {
    pub(crate) fn at(offset: u64, discard_prefix: bool) -> Self {
        Self {
            record_start: offset,
            discard_prefix,
            ..Self::default()
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.oversized || self.discard_prefix || !self.pending.is_empty()
    }

    pub(crate) fn valid_at(&self, offset: u64) -> bool {
        self.pending.len() <= MAX_RECORD_BYTES
            && self.record_start <= offset
            && if self.oversized || self.discard_prefix {
                self.pending.is_empty()
            } else {
                offset - self.record_start == self.pending.len() as u64
            }
    }

    pub(crate) fn push(
        &mut self,
        bytes: &[u8],
        offset: u64,
        mut record: impl FnMut(u64, u64, Option<&[u8]>) -> bool,
    ) -> usize {
        let mut consumed = 0;
        for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
            let complete = segment.last() == Some(&b'\n');
            let content = if complete {
                &segment[..segment.len() - 1]
            } else {
                segment
            };
            if !self.discard_prefix && !self.oversized {
                if content.len() > MAX_RECORD_BYTES.saturating_sub(self.pending.len()) {
                    self.pending.clear();
                    self.oversized = true;
                } else {
                    self.pending.extend_from_slice(content);
                }
            }
            consumed += segment.len();
            if complete {
                let end = offset + consumed as u64;
                let keep_reading = if !self.discard_prefix {
                    record(
                        self.record_start,
                        end,
                        (!self.oversized).then_some(self.pending.as_slice()),
                    )
                } else {
                    true
                };
                self.pending.clear();
                self.oversized = false;
                self.discard_prefix = false;
                self.record_start = end;
                if !keep_reading {
                    return consumed;
                }
            }
        }
        consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_boundary_stop_does_not_consume_the_next_record() {
        let mut framer = LiveJsonlFramer::at(0, false);
        let consumed = framer.push(b"first\nsecond\n", 0, |start, end, bytes| {
            assert_eq!((start, end, bytes), (0, 6, Some(b"first".as_slice())));
            false
        });
        assert_eq!(consumed, 6);
        assert!(framer.valid_at(6));
        framer.push(b"second\n", 6, |start, end, bytes| {
            assert_eq!((start, end, bytes), (6, 13, Some(b"second".as_slice())));
            true
        });
        assert!(framer.valid_at(13));
    }

    #[test]
    fn oversized_pending_records_drain_across_restart_before_later_records() {
        let mut framer = LiveJsonlFramer::at(0, false);
        let bytes = vec![b'x'; MAX_RECORD_BYTES + 1];
        framer.push(&bytes, 0, |_, _, _| panic!("the record has no newline"));
        assert!(framer.pending());
        assert!(framer.valid_at(bytes.len() as u64));
        let saved = postcard::to_allocvec(&framer).unwrap();
        assert!(saved.len() < 32);
        let mut framer: LiveJsonlFramer = postcard::from_bytes(&saved).unwrap();
        let mut records = Vec::new();
        framer.push(b"\nnext\n", bytes.len() as u64, |_, _, bytes| {
            records.push(bytes.map(<[u8]>::to_vec));
            true
        });
        assert_eq!(records, [None, Some(b"next".to_vec())]);
        assert!(!framer.pending());
    }
}
