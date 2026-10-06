//! UTF-8-safe structural ranges with bounded overlap and forward progress.

pub fn text_ranges(text: &str, window_bytes: usize, overlap_bytes: usize) -> Vec<(usize, usize)> {
    if text.len() <= window_bytes {
        return vec![(0, text.len())];
    }
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain([text.len()])
        .collect::<Vec<_>>();
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let desired_end = start.saturating_add(window_bytes).min(text.len());
        let end_index = boundaries.partition_point(|boundary| *boundary <= desired_end);
        let mut end = boundaries[end_index.saturating_sub(1)];
        if end <= start {
            end = boundaries[end_index.min(boundaries.len() - 1)];
        }
        if end < text.len()
            && let Some(relative) = text[start..end].rfind('\n')
        {
            let structural_end = start + relative + 1;
            if structural_end > start + window_bytes / 2 {
                end = structural_end;
            }
        }
        ranges.push((start, end));
        if end == text.len() {
            break;
        }
        let desired_start = end.saturating_sub(overlap_bytes);
        let start_index = boundaries.partition_point(|boundary| *boundary <= desired_start);
        start = boundaries[start_index.saturating_sub(1)];
        if ranges
            .last()
            .is_some_and(|(previous_start, _)| start <= *previous_start)
        {
            start = end;
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiny_windows_and_large_overlap_cover_every_utf8_byte() {
        for window in 0..8 {
            let text = "新\nabcé\n🦀";
            let ranges = text_ranges(text, window, 50);
            let mut covered = 0;
            for (start, end) in ranges {
                assert!(start <= covered);
                assert!(end > start);
                assert!(text.get(start..end).is_some());
                covered = end;
            }
            assert_eq!(covered, text.len());
        }
    }
}
