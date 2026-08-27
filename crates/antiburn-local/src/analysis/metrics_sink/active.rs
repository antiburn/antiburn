use std::mem::size_of;

use crate::analysis::engine::IDLE_GAP_MS;

/// The segment cap covers 1,024 distinct active-time intervals.
pub(crate) const MAX_ACTIVE_SEGMENTS: usize = 1_024;

#[derive(Clone, Copy, Debug)]
struct ActiveSegment {
    start: i64,
    end: i64,
    active_ms: i64,
}

impl ActiveSegment {
    fn exact(start: i64, end: i64) -> Self {
        Self {
            start,
            end,
            active_ms: end.saturating_sub(start).max(0),
        }
    }

    fn span_ms(self) -> i64 {
        self.end.saturating_sub(self.start).max(0)
    }

    fn measure_to(self, timestamp: i64) -> i64 {
        if timestamp <= self.start {
            return 0;
        }
        if timestamp >= self.end {
            return self.active_ms;
        }
        let span = self.span_ms();
        if span == 0 {
            return 0;
        }
        let elapsed = timestamp.saturating_sub(self.start);
        self.active_ms.saturating_mul(elapsed) / span
    }
}

#[derive(Clone, Default)]
pub(crate) struct ActiveSegments {
    segments: Vec<ActiveSegment>,
    prefix: Vec<i64>,
    prefix_valid: bool,
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    pub(crate) segments_merged: u64,
}

impl ActiveSegments {
    pub(crate) fn observe(&mut self, timestamp: i64) {
        self.first_ts = Some(
            self.first_ts
                .map_or(timestamp, |value| value.min(timestamp)),
        );
        self.last_ts = Some(self.last_ts.map_or(timestamp, |value| value.max(timestamp)));
        self.insert_segment(ActiveSegment::exact(
            timestamp,
            timestamp.saturating_add(IDLE_GAP_MS),
        ));
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        if let Some(first) = other.first_ts {
            self.first_ts = Some(self.first_ts.map_or(first, |value| value.min(first)));
        }
        if let Some(last) = other.last_ts {
            self.last_ts = Some(self.last_ts.map_or(last, |value| value.max(last)));
        }
        for &segment in &other.segments {
            self.insert_segment(segment);
        }
        self.segments_merged = self.segments_merged.saturating_add(other.segments_merged);
    }

    fn insert_segment(&mut self, incoming: ActiveSegment) {
        let first = self
            .segments
            .partition_point(|segment| segment.end < incoming.start);
        let mut last = first;
        while last < self.segments.len() && self.segments[last].start <= incoming.end {
            last += 1;
        }
        if first == last && self.segments.len() == MAX_ACTIVE_SEGMENTS {
            self.merge_overflow_segment(first, incoming);
            self.prefix_valid = false;
            return;
        }
        if first == last {
            self.segments.insert(first, incoming);
        } else {
            let mut merged = incoming;
            let existing = &self.segments[first..last];
            merged.start = merged.start.min(existing[0].start);
            merged.end = merged.end.max(existing[existing.len() - 1].end);
            if existing
                .iter()
                .all(|segment| segment.active_ms == segment.span_ms())
            {
                merged.active_ms = merged.span_ms();
            } else {
                let outside_left = existing[0].start.saturating_sub(incoming.start).max(0);
                let outside_right = incoming
                    .end
                    .saturating_sub(existing[existing.len() - 1].end)
                    .max(0);
                merged.active_ms = existing
                    .iter()
                    .fold(0_i64, |total, segment| {
                        total.saturating_add(segment.active_ms)
                    })
                    .saturating_add(outside_left)
                    .saturating_add(outside_right)
                    .min(merged.span_ms());
            }
            self.segments.splice(first..last, [merged]);
        }
        self.prefix_valid = false;
    }

    fn merge_overflow_segment(&mut self, index: usize, incoming: ActiveSegment) {
        let target = if index == 0 {
            0
        } else if index == self.segments.len() {
            index - 1
        } else {
            let left_gap = incoming.start.saturating_sub(self.segments[index - 1].end);
            let right_gap = self.segments[index].start.saturating_sub(incoming.end);
            if left_gap <= right_gap {
                index - 1
            } else {
                index
            }
        };
        let current = self.segments[target];
        self.segments[target] = ActiveSegment {
            start: current.start.min(incoming.start),
            end: current.end.max(incoming.end),
            active_ms: current.active_ms.saturating_add(incoming.active_ms),
        };
        self.segments_merged = self.segments_merged.saturating_add(1);
        tracing::debug!(event = "metrics_active_segments_capped");
    }

    pub(crate) fn rebuild_prefix(&mut self) {
        self.prefix.clear();
        self.prefix.reserve(self.segments.len());
        let mut total = 0_i64;
        for segment in &self.segments {
            self.prefix.push(total);
            total = total.saturating_add(segment.active_ms);
        }
        self.prefix_valid = true;
    }

    pub(crate) fn active_ms(&self) -> i64 {
        if self.first_ts.is_none() || self.last_ts.is_none() {
            return 0;
        }
        self.segments
            .iter()
            .fold(0_i64, |total, segment| {
                total.saturating_add(segment.active_ms)
            })
            .saturating_sub(IDLE_GAP_MS)
            .max(0)
    }

    pub(crate) fn cumulative_ms(&self, timestamp: i64) -> i64 {
        let (Some(first), Some(last)) = (self.first_ts, self.last_ts) else {
            return 0;
        };
        if timestamp <= first {
            return 0;
        }
        if timestamp >= last {
            return self.active_ms();
        }
        self.measure_between(first, timestamp)
    }

    fn measure_between(&self, start: i64, end: i64) -> i64 {
        if end <= start {
            return 0;
        }
        if self.prefix_valid {
            self.measure_to(end).saturating_sub(self.measure_to(start))
        } else {
            self.segments.iter().fold(0_i64, |total, segment| {
                total.saturating_add(
                    segment
                        .measure_to(end)
                        .saturating_sub(segment.measure_to(start)),
                )
            })
        }
    }

    fn measure_to(&self, timestamp: i64) -> i64 {
        let index = self
            .segments
            .partition_point(|segment| segment.start < timestamp);
        if index == 0 {
            return 0;
        }
        let segment_index = index - 1;
        self.prefix[segment_index]
            .saturating_add(self.segments[segment_index].measure_to(timestamp))
    }

    pub(crate) fn duration_secs(&self) -> u64 {
        match (self.first_ts, self.last_ts) {
            (Some(first), Some(last)) => ((last - first).max(0) / 1_000) as u64,
            _ => 0,
        }
    }

    pub(crate) fn earliest_ts_ms(&self) -> Option<i64> {
        self.first_ts
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.segments
            .capacity()
            .saturating_mul(size_of::<ActiveSegment>())
            .saturating_add(self.prefix.capacity().saturating_mul(size_of::<i64>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_segments_preserve_isolated_active_time() {
        let mut segments = ActiveSegments::default();
        let count = MAX_ACTIVE_SEGMENTS + 500;
        for index in 0..count {
            segments.observe(index as i64 * IDLE_GAP_MS * 2);
        }
        assert_eq!(segments.segments.len(), MAX_ACTIVE_SEGMENTS);
        segments.rebuild_prefix();
        assert_eq!(segments.prefix.len(), MAX_ACTIVE_SEGMENTS);
        assert_eq!(segments.segments_merged, 500);
        assert_eq!(segments.active_ms(), (count as i64 - 1) * IDLE_GAP_MS);
    }

    #[test]
    fn repeated_events_inside_a_compacted_gap_do_not_inflate_active_time() {
        let mut segments = ActiveSegments::default();
        for index in 0..=MAX_ACTIVE_SEGMENTS {
            segments.observe(index as i64 * IDLE_GAP_MS * 2);
        }
        let before = segments.active_ms();
        let compacted_gap = (MAX_ACTIVE_SEGMENTS as i64 - 1) * IDLE_GAP_MS * 2 + IDLE_GAP_MS + 1;
        for _ in 0..100 {
            segments.observe(compacted_gap);
        }
        assert_eq!(segments.active_ms(), before);
    }

    #[test]
    fn interval_union_matches_clamped_timestamp_gaps() {
        let mut segments = ActiveSegments::default();
        for timestamp in [10_000, 5_000, 5_000, 20_000, 1_000_000] {
            segments.observe(timestamp);
        }
        assert_eq!(segments.active_ms(), 315_000);
        assert_eq!(segments.cumulative_ms(5_000), 0);
        assert_eq!(segments.cumulative_ms(10_000), 5_000);
        assert_eq!(segments.cumulative_ms(20_000), 15_000);
        assert_eq!(segments.duration_secs(), 995);
        assert_eq!(segments.earliest_ts_ms(), Some(5_000));
    }
}
