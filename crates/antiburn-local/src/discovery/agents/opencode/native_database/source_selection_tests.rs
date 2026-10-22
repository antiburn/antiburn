use super::*;

const OLD: i64 = 1_700_000_000_000;
const RECENT: i64 = 1_800_000_000_000;

fn fixture(root: &Path, sessions: &[(&str, Option<&str>, i64)]) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let path = root.join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!(
            "../../tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    for (id, parent, updated) in sessions {
        connection
            .execute(
                "INSERT INTO session_v2 (
                     id, project_id, parent_id, slug, directory, version,
                     time_created, time_updated
                 ) VALUES (?1, 'global', ?2, 'test', '/synthetic', '2', ?3, ?3)",
                params![id, parent, updated],
            )
            .unwrap();
    }
    path
}

fn fixture_paths(root: &Path) -> Result<Vec<PathBuf>, &'static str> {
    super::super::super::database_paths_for_root_config(root, None)
}

fn select_fixture(roots: &[PathBuf], ids: &[&str]) -> HashMap<String, Option<SessionLog>> {
    select_with_path_resolver(
        roots,
        ids.iter().map(|id| (*id).to_owned()).collect(),
        Arc::new(AtomicBool::new(false)),
        fixture_paths,
    )
    .unwrap()
}

fn assert_selected(
    selected: &HashMap<String, Option<SessionLog>>,
    id: &str,
    path: &Path,
    root_id: &str,
) {
    let log = selected.get(id).unwrap().as_ref().unwrap();
    assert!(matches!(
        &log.source,
        SessionSource::ProviderDb {
            db_path,
            session_id,
            source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
            ..
        } if db_path == path && session_id == root_id
    ));
}

#[test]
fn old_first_copy_stops_before_recent_duplicate_and_corrupt_later_database() {
    let directory = tempfile::tempdir().unwrap();
    let roots: Vec<_> = ["a", "b", "c"]
        .map(|name| directory.path().join(name))
        .into();
    let preferred = fixture(&roots[0], &[("ses_root", None, OLD)]);
    fixture(&roots[1], &[("ses_root", None, RECENT)]);
    std::fs::create_dir_all(&roots[2]).unwrap();
    std::fs::write(roots[2].join("opencode.db"), b"not a database").unwrap();

    let mut selected = HashMap::new();
    assert_eq!(
        visit_databases(
            &roots,
            &BTreeSet::from(["ses_root".to_owned()]),
            &Arc::new(AtomicBool::new(false)),
            fixture_paths,
            &mut selected,
        ),
        Ok(())
    );
    assert_selected(&selected, "ses_root", &preferred, "ses_root");
    assert_eq!(
        selected["ses_root"].as_ref().unwrap().updated_at,
        Some(OLD / 1000)
    );
}

#[test]
fn resolved_ids_stop_before_later_channels_and_root_enumeration() {
    let directory = tempfile::tempdir().unwrap();
    let preferred = fixture(directory.path(), &[("ses_root", None, OLD)]);
    std::fs::write(directory.path().join("opencode-z.db"), b"not a database").unwrap();
    let roots = [
        directory.path().to_path_buf(),
        directory.path().join("skip"),
    ];
    let mut selected = HashMap::new();
    assert_eq!(
        visit_databases(
            &roots,
            &BTreeSet::from(["ses_root".to_owned()]),
            &Arc::new(AtomicBool::new(false)),
            |root| {
                assert_ne!(root.file_name().unwrap(), "skip");
                fixture_paths(root)
            },
            &mut selected,
        ),
        Ok(())
    );
    assert_selected(&selected, "ses_root", &preferred, "ses_root");
}

#[test]
fn child_root_uses_the_same_precedence_as_direct_parent_lookup() {
    let directory = tempfile::tempdir().unwrap();
    let roots: Vec<_> = ["a", "b", "c"]
        .map(|name| directory.path().join(name))
        .into();
    let preferred = fixture(&roots[0], &[("ses_parent", None, OLD)]);
    Connection::open(&preferred)
        .unwrap()
        .execute(
            "INSERT INTO session (
                 id, project_id, slug, directory, title, version, time_created, time_updated
             ) VALUES ('ses_child', 'global', 'test', '/synthetic', 'V1 child', '1', ?1, ?1)",
            [RECENT],
        )
        .unwrap();
    fixture(
        &roots[1],
        &[
            ("ses_parent", None, RECENT),
            ("ses_child", Some("ses_parent"), RECENT),
        ],
    );
    std::fs::create_dir_all(&roots[2]).unwrap();
    std::fs::write(roots[2].join("opencode.db"), b"not a database").unwrap();

    let child = select_fixture(&roots, &["ses_child"]);
    let parent = select_fixture(&roots, &["ses_parent"]);
    assert_selected(&child, "ses_child", &preferred, "ses_parent");
    assert_selected(&parent, "ses_parent", &preferred, "ses_parent");
    assert_eq!(
        child["ses_child"].as_ref().unwrap().updated_at,
        parent["ses_parent"].as_ref().unwrap().updated_at
    );
}

#[test]
fn root_selection_restarts_from_the_first_database_after_each_redirect() {
    let directory = tempfile::tempdir().unwrap();
    let roots: Vec<_> = ["a", "b", "c"]
        .map(|name| directory.path().join(name))
        .into();
    let preferred = fixture(&roots[0], &[("ses_grandparent", None, OLD)]);
    fixture(
        &roots[1],
        &[
            ("ses_grandparent", None, RECENT),
            ("ses_parent", Some("ses_grandparent"), RECENT),
        ],
    );
    fixture(
        &roots[2],
        &[
            ("ses_parent", None, RECENT),
            ("ses_child", Some("ses_parent"), RECENT),
        ],
    );

    let selected = select_fixture(&roots, &["ses_child"]);
    assert_selected(&selected, "ses_child", &preferred, "ses_grandparent");
}

#[test]
fn proven_sources_survive_an_unrelated_corrupt_database() {
    let directory = tempfile::tempdir().unwrap();
    let roots = [directory.path().join("a"), directory.path().join("b")];
    let preferred = fixture(&roots[0], &[("ses_proven", None, OLD)]);
    std::fs::create_dir_all(&roots[1]).unwrap();
    std::fs::write(roots[1].join("opencode.db"), b"not a database").unwrap();

    let selected = select_fixture(&roots, &["ses_proven", "ses_unresolved"]);
    assert_selected(&selected, "ses_proven", &preferred, "ses_proven");
    assert!(selected["ses_unresolved"].is_none());
}

#[test]
fn proven_sources_survive_an_unrelated_busy_database() {
    let directory = tempfile::tempdir().unwrap();
    let roots = [directory.path().join("a"), directory.path().join("b")];
    let preferred = fixture(&roots[0], &[("ses_proven", None, OLD)]);
    let busy_path = fixture(&roots[1], &[("ses_unresolved", None, RECENT)]);
    let busy = Connection::open(&busy_path).unwrap();
    busy.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let selected = select_fixture(&roots, &["ses_proven", "ses_unresolved"]);
    assert_selected(&selected, "ses_proven", &preferred, "ses_proven");
    assert!(selected["ses_unresolved"].is_none());
    busy.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn parent_selection_survives_errors_for_other_requested_ids() {
    let directory = tempfile::tempdir().unwrap();
    let roots: Vec<_> = ["a", "b", "c"]
        .map(|name| directory.path().join(name))
        .into();
    let preferred = fixture(&roots[0], &[("ses_parent", None, OLD)]);
    fixture(
        &roots[1],
        &[
            ("ses_parent", None, RECENT),
            ("ses_child", Some("ses_parent"), RECENT),
        ],
    );
    std::fs::create_dir_all(&roots[2]).unwrap();
    std::fs::write(roots[2].join("opencode.db"), b"not a database").unwrap();

    let selected = select_fixture(&roots, &["ses_child", "ses_unresolved"]);
    assert_selected(&selected, "ses_child", &preferred, "ses_parent");
    assert!(selected["ses_unresolved"].is_none());
}

#[test]
fn unreadable_earlier_database_blocks_unchecked_later_copies() {
    let directory = tempfile::tempdir().unwrap();
    let roots = [directory.path().join("a"), directory.path().join("b")];
    std::fs::create_dir_all(&roots[0]).unwrap();
    std::fs::write(roots[0].join("opencode.db"), b"not a database").unwrap();
    fixture(&roots[1], &[("ses_root", None, RECENT)]);

    let selected = select_fixture(&roots, &["ses_root"]);
    assert!(selected["ses_root"].is_none());
}

#[test]
fn unsafe_hierarchy_drops_only_its_candidate_without_using_a_later_copy() {
    let directory = tempfile::tempdir().unwrap();
    let roots = [directory.path().join("a"), directory.path().join("b")];
    let preferred = fixture(
        &roots[0],
        &[
            ("ses_proven", None, OLD),
            ("ses_unsafe", Some("ses_missing"), OLD),
        ],
    );
    fixture(&roots[1], &[("ses_unsafe", None, RECENT)]);

    let selected = select_fixture(&roots, &["ses_proven", "ses_unsafe", "ses_absent"]);
    assert_selected(&selected, "ses_proven", &preferred, "ses_proven");
    assert!(selected["ses_unsafe"].is_none());
    assert!(!selected.contains_key("ses_absent"));
}

#[test]
fn parent_metadata_limit_and_cycles_do_not_drop_other_candidates() {
    let directory = tempfile::tempdir().unwrap();
    let oversized = format!("ses_{}", "x".repeat(257));
    let preferred = fixture(
        directory.path(),
        &[
            ("ses_proven", None, OLD),
            ("ses_cycle_a", Some("ses_cycle_b"), OLD),
            ("ses_cycle_b", Some("ses_cycle_a"), OLD),
            ("ses_oversized", Some(&oversized), OLD),
        ],
    );

    let selected = select_fixture(
        &[directory.path().to_path_buf()],
        &["ses_proven", "ses_cycle_a", "ses_oversized"],
    );
    assert_selected(&selected, "ses_proven", &preferred, "ses_proven");
    assert!(selected["ses_cycle_a"].is_none());
    assert!(selected["ses_oversized"].is_none());
}

#[test]
fn local_parent_resolution_has_a_depth_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = fixture(directory.path(), &[]);
    let connection = Connection::open(path).unwrap();
    for index in 0..=MAX_V2_PARENT_DEPTH + 1 {
        let id = format!("ses_{index}");
        let parent = (index <= MAX_V2_PARENT_DEPTH).then(|| format!("ses_{}", index + 1));
        connection
            .execute(
                "INSERT INTO session_v2 (
                     id, project_id, parent_id, slug, directory, version,
                     time_created, time_updated
                 ) VALUES (?1, 'global', ?2, 'test', '/synthetic', '2', ?3, ?3)",
                params![id, parent, OLD],
            )
            .unwrap();
    }

    let selected = select_fixture(&[directory.path().to_path_buf()], &["ses_0"]);
    assert!(selected["ses_0"].is_none());
}

#[test]
fn cross_database_root_redirects_have_a_depth_limit() {
    let directory = tempfile::tempdir().unwrap();
    let mut roots = Vec::new();
    for index in 0..=MAX_V2_PARENT_DEPTH + 1 {
        let root = directory.path().join(format!("root_{index}"));
        let id = format!("ses_{index}");
        if index == 0 {
            fixture(&root, &[(&id, None, OLD)]);
        } else {
            let parent = format!("ses_{}", index - 1);
            fixture(&root, &[(&id, Some(&parent), OLD), (&parent, None, OLD)]);
        }
        roots.push(root);
    }

    let id = format!("ses_{}", MAX_V2_PARENT_DEPTH + 1);
    let selected = select_fixture(&roots, &[&id]);
    assert!(selected[&id].is_none());
}

#[test]
fn malformed_later_schema_preserves_proven_sources() {
    let directory = tempfile::tempdir().unwrap();
    let roots = [directory.path().join("a"), directory.path().join("b")];
    let preferred = fixture(&roots[0], &[("ses_proven", None, OLD)]);
    let malformed = fixture(&roots[1], &[("ses_unresolved", None, RECENT)]);
    Connection::open(malformed)
        .unwrap()
        .execute_batch("DROP INDEX session_message_session_seq_idx")
        .unwrap();

    let selected = select_fixture(&roots, &["ses_proven", "ses_unresolved"]);
    assert_selected(&selected, "ses_proven", &preferred, "ses_proven");
    assert!(selected["ses_unresolved"].is_none());
}

#[test]
fn cancellation_stops_source_selection() {
    let directory = tempfile::tempdir().unwrap();
    let result = select_with_path_resolver(
        &[directory.path().to_path_buf()],
        HashSet::from(["ses_root".to_owned()]),
        Arc::new(AtomicBool::new(true)),
        |_| panic!("cancelled selection must not enumerate database paths"),
    );
    assert_eq!(result.unwrap_err(), "cancelled");
}
