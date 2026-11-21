#![cfg(all(unix, feature = "cloud"))]

//! Bounded filename paging through synthetic Claude sources.

use antiburn_cloud_capture::{Limits, discover_claude, discover_claude_page};
use std::collections::BTreeSet;
use std::fs;

#[test]
fn pages_main_and_child_sources_without_duplicates_or_payload_reads() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let root = temp.path();
    for index in (0..600).rev() {
        fs::write(root.join(format!("session-{index:04}.jsonl")), b"not JSON").unwrap();
    }
    fs::create_dir_all(root.join("session-0000/subagents")).unwrap();
    fs::write(
        root.join("session-0000/subagents/agent-child.jsonl"),
        b"not JSON",
    )
    .unwrap();
    let limits = Limits {
        discovery_sessions: 127,
        ..Limits::default()
    };
    assert!(discover_claude(root, &limits).is_err());
    let mut after = None;
    let mut ids = BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = discover_claude_page(root, after.as_deref(), &limits).unwrap();
        assert!(page.candidates.len() <= 127);
        for candidate in page.candidates {
            assert!(
                after
                    .as_deref()
                    .is_none_or(|after| candidate.source_id() > after)
            );
            assert!(ids.insert(candidate.source_id().to_owned()));
        }
        pages += 1;
        after = page.next_cursor;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(pages, 5);
    assert_eq!(ids.len(), 601);
    assert!(ids.contains("session-0000/subagents/agent-child"));
}
