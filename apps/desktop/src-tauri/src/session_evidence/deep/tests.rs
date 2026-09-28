use super::*;
use crate::session_search::SessionSearchEntry;

#[test]
fn phrase_search_requires_the_complete_phrase() {
    assert!(
        find_match(
            "fixed deterministic search",
            "fixed search",
            &[],
            Some("fixed search")
        )
        .is_none()
    );
    assert_eq!(
        find_match(
            "fixed search result",
            "fixed search",
            &[],
            Some("fixed search")
        ),
        Some((0, 12, 2_000))
    );
}

#[test]
fn term_search_scores_complete_matches_above_partial_matches() {
    let terms = vec!["cedar".to_owned(), "module".to_owned()];
    let complete = find_match("cedar module failed", "cedar module", &terms, None).unwrap();
    let partial = find_match("cedar failed", "cedar module", &terms, None).unwrap();
    assert!(complete.2 > partial.2);
}

#[test]
fn excerpts_keep_the_match_and_report_truncation() {
    let text = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
    let (excerpt, truncated) = excerpt_around(&text, 300, 306);
    assert!(truncated);
    assert!(excerpt.contains("needle"));
    assert!(excerpt.starts_with('…'));
    assert!(excerpt.ends_with('…'));
}

#[test]
fn equal_rank_results_use_the_complete_session_identity() {
    fn hit(environment_key: &str, agent: &str) -> SessionEvidenceHit {
        let stored = StoredEvidenceReference {
            environment_key: environment_key.into(),
            agent: agent.into(),
            session_id: "shared".into(),
            source_generation: 1,
            published_fence: 1,
            source_key: "source".into(),
            thread_id: "main".into(),
            scope: "main".into(),
            turn_rowid: 1,
            turn_index: 0,
            part_index: 0,
        };
        SessionEvidenceHit {
            session: SessionSearchEntry {
                environment_key: environment_key.into(),
                agent: agent.into(),
                session_id: "shared".into(),
                wsl_distro: None,
                title: None,
                repository: String::new(),
                cwd_label: String::new(),
                models: Vec::new(),
                timestamp: "2026-09-28T00:00:00Z".into(),
            },
            reference: evidence_reference(stored, b"same"),
            excerpt: "same".into(),
            kind: EvidenceKind::Assistant,
            score: 1,
            retrieval_rank: 0,
            coverage: EvidenceCoverage {
                state: "complete",
                inspected_bytes: 4,
                byte_limit: 4,
            },
            truncated: false,
        }
    }

    let mut hits = [
        hit("wsl:ubuntu", "codex"),
        hit("native", "pi"),
        hit("native", "codex"),
    ];
    hits.sort_by(compare_hits);
    assert_eq!(
        hits.map(|hit| (hit.session.environment_key, hit.session.agent)),
        [
            ("native".into(), "codex".into()),
            ("native".into(), "pi".into()),
            ("wsl:ubuntu".into(), "codex".into()),
        ]
    );
}
