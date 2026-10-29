use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    remote: PathBuf,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let home = root.join("home");
        let remote = root.join("remote-sessions");
        std::fs::create_dir(&home).unwrap();
        let store = Store::open_in_memory(&root).unwrap();
        Self {
            _directory: directory,
            root,
            home,
            remote,
            store,
        }
    }

    fn context(&self) -> DirectoryContext {
        context(
            &self.store,
            self.home.clone(),
            RemoteCache {
                path: self.remote.clone(),
                canonical: self.remote.clone(),
            },
        )
        .unwrap()
    }

    fn global(&self) -> PathBuf {
        self.home.join(".antiburn/checks")
    }

    fn scan(&self) -> (Vec<CheckDirectory>, Bindings) {
        scan(&self.context(), &Bindings::new()).unwrap()
    }

    fn create_global(&self) {
        std::fs::create_dir_all(self.global()).unwrap();
    }
}

fn repository(root: &Path) -> crate::store::RepositoryRecord {
    crate::store::RepositoryRecord {
        key: root.to_string_lossy().into_owned(),
        repo_name: "project".into(),
        full_name: "synthetic/project".into(),
        status: "accessible".into(),
        repo_root: Some(root.to_string_lossy().into_owned()),
        suspected_path: None,
        worktree_count: 1,
        session_count: 0,
        wsl_distro: None,
        enabled: true,
    }
}

#[test]
fn physical_file_counts_include_invalid_shadowed_and_collision_files_without_reading_contents() {
    let fixture = Fixture::new();
    fixture.create_global();
    let project = fixture.root.join("project");
    let project_checks = project.join(".antiburn/checks");
    std::fs::create_dir_all(&project_checks).unwrap();
    fixture
        .store
        .replace_repositories(&[repository(&project)])
        .unwrap();
    for name in ["same.yml", "same.yaml", "Bad_Name.yml", "other.yml"] {
        std::fs::write(fixture.global().join(name), [0xff; 16]).unwrap();
    }
    std::fs::write(project_checks.join("same.yml"), "invalid: [").unwrap();
    std::fs::write(fixture.global().join("large.yaml"), vec![0xff; 300 * 1024]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            fixture.global().join("other.yml"),
            std::fs::Permissions::from_mode(0o0),
        )
        .unwrap();
    }
    std::fs::write(fixture.global().join("ignored.YAML"), "name: ignored").unwrap();
    std::fs::write(fixture.global().join("ignored.txt"), "name: ignored").unwrap();
    std::fs::create_dir(fixture.global().join("folder.yml")).unwrap();
    std::fs::create_dir(fixture.global().join("nested")).unwrap();
    std::fs::write(fixture.global().join("nested/ignored.yml"), "name: ignored").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.global().join("same.yml"),
        fixture.global().join("link.yml"),
    )
    .unwrap();
    let (rows, bindings) = fixture.scan();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].file_count, Some(5));
    assert_eq!(rows[1].file_count, Some(1));
    assert!(rows.iter().all(|row| row.state == DirectoryState::Ready));
    assert_eq!(bindings.len(), 2);
}

#[test]
fn off_state_inventory_needs_no_credentials_sessions_or_runtime_metadata() {
    let fixture = Fixture::new();
    fixture.create_global();
    fixture
        .store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
        .unwrap();
    assert!(!crate::jev::settings::yaml_discovery_enabled(
        &fixture.store
    ));
    let roots = [
        fixture.root.join("one/project"),
        fixture.root.join("two/project"),
    ];
    for root in &roots {
        std::fs::create_dir_all(root.join(".antiburn/checks")).unwrap();
    }
    fixture
        .store
        .replace_repositories(
            &roots
                .iter()
                .map(|root| repository(root))
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let state = super::super::InstalledChecks::default();
    let (rows, bindings) = fixture.scan();
    assert_eq!(rows.len(), 3);
    assert_ne!(rows[1].path, rows[2].path);
    assert!(rows.iter().all(|row| row.file_count == Some(0)));
    assert_eq!(bindings.len(), 3);
    assert!(state.metadata.read().unwrap().is_empty());
    assert!(!state.credentials_restored.load(Ordering::Acquire));
    assert_eq!(state.requested_turn.load(Ordering::Acquire), 0);
    assert!(!crate::jev::settings::yaml_discovery_enabled(
        &fixture.store
    ));
}

#[test]
fn inventory_distinguishes_missing_empty_paused_and_non_directory_without_creating_paths() {
    let fixture = Fixture::new();
    let (missing, bindings) = fixture.scan();
    assert_eq!(missing[0].state, DirectoryState::Missing);
    assert_eq!(missing[0].file_count, None);
    assert!(bindings.is_empty());
    assert!(!fixture.global().exists());
    fixture.create_global();
    let (empty, _) = fixture.scan();
    assert_eq!(empty[0].state, DirectoryState::Ready);
    assert_eq!(empty[0].file_count, Some(0));
    std::fs::remove_dir(fixture.global()).unwrap();
    std::fs::write(fixture.global(), "not a directory").unwrap();
    let project = fixture.root.join("disabled-project");
    let mut disabled = repository(&project);
    disabled.enabled = false;
    fixture.store.replace_repositories(&[disabled]).unwrap();
    let (rows, bindings) = fixture.scan();
    assert_eq!(rows[0].state, DirectoryState::Unavailable);
    assert_eq!(rows[1].state, DirectoryState::Paused);
    assert!(
        rows.iter()
            .all(|row| row.file_count.is_none() && row.reference.is_none())
    );
    assert!(bindings.is_empty());
    assert!(!project.exists());
}

#[test]
fn candidate_policy_excludes_relative_and_wsl_roots_and_does_not_read_session_scopes() {
    let fixture = Fixture::new();
    let root = fixture.root.join("project");
    let mut wsl = repository(&fixture.root.join("wsl-project"));
    wsl.wsl_distro = Some("synthetic-distro".into());
    let records = vec![
        repository(&root),
        wsl,
        repository(Path::new("relative")),
        repository(&root),
    ];
    let candidates = candidates(&fixture.home, super::super::native_repositories(records)).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[1].path, root.join(".antiburn/checks"));
    assert!(super::candidates(Path::new("relative-home"), BTreeMap::new()).is_err());
}

#[test]
fn incomplete_or_over_limit_listings_have_unknown_counts_not_partial_totals() {
    let fixture = Fixture::new();
    fixture.create_global();
    for index in 0..super::super::MAX_DEFINITIONS {
        std::fs::write(fixture.global().join(format!("check-{index}.yml")), "").unwrap();
    }
    assert_eq!(
        fixture.scan().0[0].file_count,
        Some(super::super::MAX_DEFINITIONS)
    );
    let entries = std::fs::read_dir(fixture.global())
        .unwrap()
        .take(1)
        .chain(std::iter::once(Err(std::io::Error::other(
            "synthetic iteration failure",
        ))));
    assert!(count_files(entries).is_err());
    std::fs::write(fixture.global().join("one-more.yml"), "").unwrap();
    let (rows, bindings) = fixture.scan();
    assert_eq!(rows[0].state, DirectoryState::Unavailable);
    assert_eq!(rows[0].file_count, None);
    assert!(bindings.is_empty());
}

#[test]
fn unrelated_entries_and_candidate_directories_have_separate_work_bounds() {
    let fixture = Fixture::new();
    fixture.create_global();
    for index in 0..=MAX_DIRECTORY_ENTRIES {
        std::fs::write(fixture.global().join(format!("entry-{index}.txt")), "").unwrap();
    }
    assert_eq!(fixture.scan().0[0].file_count, None);
    let records = (0..MAX_DIRECTORIES)
        .map(|index| repository(&fixture.root.join(format!("project-{index}"))))
        .collect();
    assert!(candidates(&fixture.home, super::super::native_repositories(records)).is_err());
}

#[test]
fn repository_metadata_reads_stop_at_the_row_bound_even_when_roots_repeat() {
    let fixture = Fixture::new();
    let mut records = (0..MAX_REPOSITORY_ROWS)
        .map(|index| {
            let mut record = repository(&fixture.root.join("one-project"));
            record.key = format!("synthetic-key-{index}");
            record
        })
        .collect::<Vec<_>>();
    fixture.store.replace_repositories(&records).unwrap();
    assert_eq!(known_native_repositories(&fixture.store).unwrap().len(), 1);
    let mut overflow = repository(&fixture.root.join("wsl-project"));
    overflow.key = "synthetic-overflow".into();
    overflow.repo_root = None;
    overflow.wsl_distro = Some("synthetic-distro".into());
    records.push(overflow);
    fixture.store.replace_repositories(&records).unwrap();
    assert!(known_native_repositories(&fixture.store).is_err());
    assert!(!fixture.root.join("one-project").exists());
}

#[test]
fn repository_query_uses_the_primary_key_index_without_a_temporary_sort() {
    let fixture = Fixture::new();
    let connection = fixture.store.lock();
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {REPOSITORY_ROWS_QUERY}"))
        .unwrap();
    let details = statement
        .query_map([MAX_REPOSITORY_ROWS + 1], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        details
            .iter()
            .any(|detail| detail.contains("USING INDEX sqlite_autoindex_repository_1")),
        "{details:?}"
    );
    assert!(
        details.iter().all(|detail| !detail.contains("TEMP")),
        "{details:?}"
    );
}

#[test]
fn repository_query_work_stays_bounded_when_excluded_records_grow() {
    fn query_work(record_count: usize) -> (usize, i32, i32) {
        let fixture = Fixture::new();
        let records = (0..record_count)
            .map(|index| {
                let mut record = repository(&fixture.root.join("wsl-project"));
                record.key = format!("synthetic-key-{index:05}");
                record.full_name = format!("synthetic/{:05}", record_count - index);
                record.wsl_distro = Some("synthetic-distro".into());
                record
            })
            .collect::<Vec<_>>();
        fixture.store.replace_repositories(&records).unwrap();
        let connection = fixture.store.lock();
        let mut statement = connection.prepare(REPOSITORY_ROWS_QUERY).unwrap();
        let count = statement
            .query_map([MAX_REPOSITORY_ROWS + 1], |_| Ok(()))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .len();
        (
            count,
            statement.get_status(rusqlite::StatementStatus::VmStep),
            statement.get_status(rusqlite::StatementStatus::Sort),
        )
    }
    let small = query_work(MAX_REPOSITORY_ROWS + 1);
    let large = query_work(MAX_REPOSITORY_ROWS * 2);
    assert_eq!(small.0, MAX_REPOSITORY_ROWS + 1);
    assert!(small.1 > 0);
    assert_eq!(small.2, 0);
    assert_eq!(large, small);
}

#[test]
fn bounded_repository_rows_keep_runtime_filters_and_full_name_key_duplicate_precedence() {
    let fixture = Fixture::new();
    let first_root = fixture.root.join("first-project");
    let second_root = fixture.root.join("second-project");
    let mut last_name = repository(&first_root);
    last_name.key = "a-last-name".into();
    last_name.full_name = "z-group".into();
    last_name.enabled = false;
    let mut first_name = repository(&first_root);
    first_name.key = "z-first-name".into();
    first_name.full_name = "a-group".into();
    let mut first_key = repository(&second_root);
    first_key.key = "a-same-name".into();
    first_key.full_name = "same-name".into();
    let mut last_key = repository(&second_root);
    last_key.key = "z-same-name".into();
    last_key.full_name = "same-name".into();
    last_key.enabled = false;
    let mut wsl = repository(&first_root);
    wsl.key = "wsl-record".into();
    wsl.full_name = "zz-group".into();
    wsl.wsl_distro = Some("synthetic-distro".into());
    let mut relative = repository(Path::new("relative-project"));
    relative.key = "relative-record".into();
    let mut missing = repository(&fixture.root.join("missing-root"));
    missing.key = "missing-record".into();
    missing.repo_root = None;
    fixture
        .store
        .replace_repositories(&[
            last_key, first_name, relative, wsl, last_name, missing, first_key,
        ])
        .unwrap();
    let expected = super::super::native_repositories(fixture.store.repositories().unwrap());
    let actual = known_native_repositories(&fixture.store).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 2);
    assert_eq!(actual.get(&first_root), Some(&false));
    assert_eq!(actual.get(&second_root), Some(&false));
}

#[test]
fn references_are_stable_for_unchanged_bindings_and_failed_refresh_keeps_the_last_binding() {
    let fixture = Fixture::new();
    fixture.create_global();
    let inventory = DirectoryInventory::default();
    let first = inventory.complete(Ok(fixture.scan())).unwrap();
    let reference = first[0].reference.clone().unwrap();
    let binding = inventory.binding(&reference).unwrap();
    let refreshed = scan(
        &fixture.context(),
        &inventory.snapshot.read().unwrap().bindings,
    )
    .unwrap();
    assert_eq!(
        refreshed.0[0].reference.as_deref(),
        Some(reference.as_str())
    );
    inventory.complete(Err(READ_ERROR.into())).unwrap_err();
    assert_eq!(inventory.binding(&reference), Some(binding));
    assert!(
        inventory
            .snapshot
            .read()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .is_err()
    );
    std::fs::remove_dir(fixture.global()).unwrap();
    inventory.complete(Ok(fixture.scan())).unwrap();
    assert!(inventory.binding(&reference).is_none());
}

#[tokio::test]
async fn overlapping_reads_share_one_bounded_worker_and_later_reads_refresh() {
    let inventory = Arc::new(DirectoryInventory::default());
    let guard = inventory.read_lock.clone().lock_owned().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    let mut started = Vec::new();
    for _ in 0..2 {
        let inventory = Arc::clone(&inventory);
        let reads = Arc::clone(&reads);
        let (send, receive) = tokio::sync::oneshot::channel();
        started.push(receive);
        tasks.push(tokio::spawn(async move {
            send.send(()).unwrap();
            read_inventory(inventory, move |_| {
                reads.fetch_add(1, Ordering::AcqRel);
                Ok((Vec::new(), Bindings::new()))
            })
            .await
        }));
    }
    for started in started {
        started.await.unwrap();
    }
    drop(guard);
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(reads.load(Ordering::Acquire), 1);
    let later_reads = Arc::clone(&reads);
    read_inventory(inventory, move |_| {
        later_reads.fetch_add(1, Ordering::AcqRel);
        Ok((Vec::new(), Bindings::new()))
    })
    .await
    .unwrap();
    assert_eq!(reads.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn cancelling_a_reader_does_not_release_its_worker_or_allow_old_results_to_overwrite_new_results()
 {
    let inventory = Arc::new(DirectoryInventory::default());
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let first_inventory = Arc::clone(&inventory);
    let first = tokio::spawn(async move {
        read_inventory(first_inventory, move |_| {
            entered.send(()).unwrap();
            held.recv().unwrap();
            Ok((Vec::new(), Bindings::new()))
        })
        .await
    });
    waiting.await.unwrap();
    first.abort();
    assert!(inventory.read_lock.try_lock().is_err());
    let (started, queued) = tokio::sync::oneshot::channel();
    let second_inventory = Arc::clone(&inventory);
    let second = tokio::spawn(async move {
        started.send(()).unwrap();
        read_inventory(second_inventory, |_| {
            panic!("The overlapping read must reuse the completed worker")
        })
        .await
    });
    queued.await.unwrap();
    release.send(()).unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(inventory.generation(), 1);
    read_inventory(Arc::clone(&inventory), |_| Err("newer read failed".into()))
        .await
        .unwrap_err();
    assert_eq!(inventory.generation(), 2);
    assert_eq!(
        inventory
            .snapshot
            .read()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err(),
        "newer read failed"
    );
}

#[test]
fn folder_open_uses_only_the_issued_exact_canonical_container_as_one_literal_argument() {
    let fixture = Fixture::new();
    let root = fixture.root.join("- project 'quoted' ; $value Unicode-λ");
    let checks = root.join(".antiburn/checks");
    std::fs::create_dir_all(&checks).unwrap();
    fixture
        .store
        .replace_repositories(&[repository(&root)])
        .unwrap();
    let (rows, bindings) = fixture.scan();
    let reference = rows[1].reference.as_ref().unwrap();
    let mut opened = Vec::new();
    open_directory(
        bindings.get(reference),
        &fixture.context(),
        || true,
        |path| {
            opened.push(path);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        opened,
        [
            crate::commands::presentable(std::fs::canonicalize(checks).unwrap())
                .into_os_string()
                .into_string()
                .unwrap()
        ]
    );
    assert!(!fixture.global().exists());
    for reference in [
        "unknown-reference",
        rows[1].path.as_str(),
        root.to_str().unwrap(),
        "wsl:synthetic",
        "ssh:synthetic",
    ] {
        assert_eq!(
            open_directory(
                bindings.get(reference),
                &fixture.context(),
                || true,
                |_| panic!("Unissued reference")
            )
            .unwrap_err()
            .outcome,
            BurnCheckDirectoryOpenOutcome::Missing
        );
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_native_paths_cannot_become_display_path_arguments() {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(
        b"/home-\xff/.antiburn/checks".to_vec(),
    ));
    assert!(path.to_string_lossy().contains(char::REPLACEMENT_CHARACTER));
    assert_eq!(
        open_path_argument(path).unwrap_err().outcome,
        BurnCheckDirectoryOpenOutcome::Unavailable
    );
}

// macOS rejects non-UTF-8 filenames before the inventory can read them.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn lossy_display_paths_never_replace_native_path_authority() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let home = fixture
        .root
        .join(std::ffi::OsString::from_vec(b"home-\xff".to_vec()));
    std::fs::create_dir_all(home.join(".antiburn/checks")).unwrap();
    let context = context(
        &fixture.store,
        home,
        RemoteCache {
            path: fixture.remote.clone(),
            canonical: fixture.remote.clone(),
        },
    )
    .unwrap();
    let (rows, bindings) = scan(&context, &Bindings::new()).unwrap();
    assert!(rows[0].path.contains(char::REPLACEMENT_CHARACTER));
    let binding = bindings.get(rows[0].reference.as_ref().unwrap());
    assert_eq!(
        open_directory(
            binding,
            &context,
            || true,
            |_| panic!("A lossy display path is not an opener argument")
        )
        .unwrap_err()
        .outcome,
        BurnCheckDirectoryOpenOutcome::Unavailable
    );
}

#[test]
fn open_rechecks_current_repository_membership_enabled_state_policy_and_directory_type() {
    let fixture = Fixture::new();
    let root = fixture.root.join("project");
    let checks = root.join(".antiburn/checks");
    std::fs::create_dir_all(&checks).unwrap();
    let record = repository(&root);
    fixture
        .store
        .replace_repositories(std::slice::from_ref(&record))
        .unwrap();
    let (_, bindings) = fixture.scan();
    let binding = bindings.values().next().unwrap();
    let earlier_policy = fixture.context();
    assert!(policy_is_current(&fixture.store, &earlier_policy));
    assert!(
        open_directory(
            Some(binding),
            &fixture.context(),
            || false,
            |_| panic!("Changed policy")
        )
        .is_err()
    );
    fixture
        .store
        .set_repository_enabled(&record.key, false)
        .unwrap();
    assert!(!policy_is_current(&fixture.store, &earlier_policy));
    assert!(
        open_directory(
            Some(binding),
            &fixture.context(),
            || true,
            |_| panic!("Disabled repository")
        )
        .is_err()
    );
    let paused_policy = fixture.context();
    fixture.store.replace_repositories(&[]).unwrap();
    assert!(!policy_is_current(&fixture.store, &paused_policy));
    assert!(
        open_directory(
            Some(binding),
            &fixture.context(),
            || true,
            |_| panic!("Removed repository")
        )
        .is_err()
    );
    fixture.store.replace_repositories(&[record]).unwrap();
    fixture
        .store
        .set_repository_enabled(&root.to_string_lossy(), true)
        .unwrap();
    std::fs::remove_dir(&checks).unwrap();
    assert_eq!(
        open_directory(
            Some(binding),
            &fixture.context(),
            || true,
            |_| panic!("Missing directory")
        )
        .unwrap_err()
        .outcome,
        BurnCheckDirectoryOpenOutcome::Missing
    );
    assert!(!checks.exists());
    std::fs::write(&checks, "replaced with a file").unwrap();
    assert_eq!(
        open_directory(
            Some(binding),
            &fixture.context(),
            || true,
            |_| panic!("Not a directory")
        )
        .unwrap_err()
        .outcome,
        BurnCheckDirectoryOpenOutcome::Unavailable
    );
}

#[cfg(unix)]
#[test]
fn a_preexisting_directory_symlink_can_open_but_changed_targets_get_new_references() {
    let fixture = Fixture::new();
    let first = fixture.root.join("first");
    let second = fixture.root.join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::fs::create_dir(fixture.home.join(".antiburn")).unwrap();
    std::os::unix::fs::symlink(&first, fixture.global()).unwrap();
    let (rows, bindings) = fixture.scan();
    let reference = rows[0].reference.as_ref().unwrap();
    open_directory(
        bindings.get(reference),
        &fixture.context(),
        || true,
        |path| {
            assert_eq!(Path::new(&path), first);
            Ok(())
        },
    )
    .unwrap();
    std::fs::remove_file(fixture.global()).unwrap();
    std::os::unix::fs::symlink(&second, fixture.global()).unwrap();
    assert!(
        open_directory(
            bindings.get(reference),
            &fixture.context(),
            || true,
            |_| panic!("Redirected directory")
        )
        .is_err()
    );
    let (rows, next) = scan(&fixture.context(), &bindings).unwrap();
    assert_ne!(rows[0].reference.as_ref(), Some(reference));
    assert!(!next.contains_key(reference));
}

#[test]
fn unresolvable_or_non_directory_home_policy_fails_closed_without_creating_folders() {
    let fixture = Fixture::new();
    let file_home = fixture.root.join("home-file");
    std::fs::write(&file_home, "not a home directory").unwrap();
    let missing_home = fixture.root.join("missing-home");
    for home in [
        PathBuf::from("relative-home"),
        missing_home.clone(),
        file_home,
    ] {
        assert!(
            context(
                &fixture.store,
                home,
                RemoteCache {
                    path: fixture.remote.clone(),
                    canonical: fixture.remote.clone()
                }
            )
            .is_err()
        );
    }
    assert!(!missing_home.exists());
    assert!(!fixture.global().exists());
    #[cfg(unix)]
    {
        let broken_home = fixture.root.join("broken-home");
        std::os::unix::fs::symlink(&missing_home, &broken_home).unwrap();
        assert!(
            context(
                &fixture.store,
                broken_home,
                RemoteCache {
                    path: fixture.remote.clone(),
                    canonical: fixture.remote.clone()
                }
            )
            .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn changed_or_unresolvable_home_alias_invalidates_the_captured_policy() {
    let fixture = Fixture::new();
    let actual_home = fixture.root.join("actual-home");
    let other_home = fixture.root.join("other-home");
    std::fs::rename(&fixture.home, &actual_home).unwrap();
    std::fs::create_dir(&other_home).unwrap();
    std::os::unix::fs::symlink(&actual_home, &fixture.home).unwrap();
    let captured = fixture.context();
    assert!(policy_is_current(&fixture.store, &captured));
    std::fs::remove_file(&fixture.home).unwrap();
    assert!(!policy_is_current(&fixture.store, &captured));
    std::os::unix::fs::symlink(&other_home, &fixture.home).unwrap();
    assert!(!policy_is_current(&fixture.store, &captured));
    assert_eq!(fixture.context().canonical_home, other_home);
}

#[cfg(target_os = "macos")]
#[test]
fn symlinked_home_checks_both_home_paths_and_revoked_grants_block_each_issued_folder() {
    let fixture = Fixture::new();
    let actual_home = fixture.root.join("actual-home");
    std::fs::rename(&fixture.home, &actual_home).unwrap();
    std::os::unix::fs::symlink(&actual_home, &fixture.home).unwrap();
    let global_target = actual_home.join("Documents/check-files");
    std::fs::create_dir_all(&global_target).unwrap();
    std::fs::write(global_target.join("synthetic.yml"), "").unwrap();
    std::fs::create_dir(actual_home.join(".antiburn")).unwrap();
    std::os::unix::fs::symlink(&global_target, fixture.global()).unwrap();
    let project_root = actual_home.join("Documents/project");
    fixture
        .store
        .replace_repositories(&[repository(&project_root)])
        .unwrap();
    let ungranted = fixture.context();
    assert_eq!(ungranted.home, fixture.home);
    assert_eq!(ungranted.canonical_home, actual_home);
    for name in protected::protected_dir_names() {
        assert!(ungranted.blocked(&fixture.home.join(name).join("project")));
        assert!(ungranted.blocked(&actual_home.join(name).join("project")));
    }
    let (rows, bindings) = scan(&ungranted, &Bindings::new()).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.state == DirectoryState::Unavailable
                && row.file_count.is_none()
                && row.reference.is_none()
                && row.message == Some(ACCESS_MESSAGE))
    );
    assert!(bindings.is_empty());
    assert!(!project_root.exists());
    assert!(fixture.store.granted_dirs().unwrap().is_empty());
    let project_checks = project_root.join(".antiburn/checks");
    std::fs::create_dir_all(&project_checks).unwrap();
    fixture.store.grant_dir("Documents").unwrap();
    let granted = fixture.context();
    let (rows, bindings) = scan(&granted, &Bindings::new()).unwrap();
    assert!(
        rows.iter()
            .all(|row| row.state == DirectoryState::Ready && row.reference.is_some())
    );
    assert_eq!(rows[0].file_count, Some(1));
    assert_eq!(rows[1].file_count, Some(0));
    let global_binding = bindings.get(rows[0].reference.as_ref().unwrap()).unwrap();
    assert_eq!(global_binding.candidate.path, fixture.global());
    assert_eq!(global_binding.canonical, global_target);
    assert_ne!(global_binding.candidate.path, global_binding.canonical);
    let mut opened = Vec::new();
    for row in &rows {
        open_directory(
            bindings.get(row.reference.as_ref().unwrap()),
            &granted,
            || policy_is_current(&fixture.store, &granted),
            |path| {
                opened.push(PathBuf::from(path));
                Ok(())
            },
        )
        .unwrap();
    }
    assert_eq!(opened, [global_target, project_checks]);
    assert_eq!(
        fixture.store.granted_dirs().unwrap(),
        HashSet::from(["Documents".into()])
    );
    fixture.store.revoke_dir_grant("Documents").unwrap();
    let revoked = fixture.context();
    assert!(!policy_is_current(&fixture.store, &granted));
    let (unavailable, new_bindings) = scan(&revoked, &bindings).unwrap();
    assert!(
        unavailable
            .iter()
            .all(|row| row.state == DirectoryState::Unavailable
                && row.file_count.is_none()
                && row.reference.is_none())
    );
    assert!(new_bindings.is_empty());
    for row in &rows {
        assert_eq!(
            open_directory(
                bindings.get(row.reference.as_ref().unwrap()),
                &revoked,
                || true,
                |_| panic!("A revoked grant must prevent the opener")
            )
            .unwrap_err()
            .outcome,
            BurnCheckDirectoryOpenOutcome::Unavailable
        );
    }
    assert!(fixture.store.granted_dirs().unwrap().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn ungranted_paths_are_not_inspected_and_revoked_or_canonical_target_grants_block_open() {
    let fixture = Fixture::new();
    let root = fixture.home.join("Documents/project");
    fixture
        .store
        .replace_repositories(&[repository(&root)])
        .unwrap();
    let context = fixture.context();
    let rows = scan(&context, &Bindings::new()).unwrap().0;
    assert_eq!(rows[1].state, DirectoryState::Unavailable);
    assert_eq!(rows[1].file_count, None);
    assert_eq!(rows[1].message, Some(ACCESS_MESSAGE));
    assert!(!root.exists());
    let checks = root.join(".antiburn/checks");
    std::fs::create_dir_all(&checks).unwrap();
    fixture.store.grant_dir("Documents").unwrap();
    let granted = fixture.context();
    let (rows, bindings) = scan(&granted, &Bindings::new()).unwrap();
    let binding = bindings.get(rows[1].reference.as_ref().unwrap());
    fixture.store.revoke_dir_grant("Documents").unwrap();
    assert!(!policy_is_current(&fixture.store, &granted));
    assert!(
        open_directory(
            binding,
            &fixture.context(),
            || true,
            |_| panic!("Revoked grant")
        )
        .is_err()
    );
    std::fs::create_dir(fixture.home.join(".antiburn")).unwrap();
    std::os::unix::fs::symlink(&checks, fixture.global()).unwrap();
    assert_eq!(
        scan(&fixture.context(), &Bindings::new()).unwrap().0[0].message,
        Some(ACCESS_MESSAGE)
    );
    fixture.store.grant_dir("Documents").unwrap();
    let granted = fixture.context();
    let (rows, bindings) = scan(&granted, &Bindings::new()).unwrap();
    fixture.store.revoke_dir_grant("Documents").unwrap();
    assert!(
        open_directory(
            bindings.get(rows[0].reference.as_ref().unwrap()),
            &fixture.context(),
            || true,
            |_| panic!("Canonical target grant")
        )
        .is_err()
    );
}

#[test]
fn remote_cache_folders_have_no_issued_reference_and_cannot_open_even_with_a_binding() {
    let fixture = Fixture::new();
    let root = fixture.remote.join("host/project");
    let checks = root.join(".antiburn/checks");
    std::fs::create_dir_all(&checks).unwrap();
    fixture
        .store
        .replace_repositories(&[repository(&root)])
        .unwrap();
    let (rows, bindings) = fixture.scan();
    assert_eq!(rows[1].state, DirectoryState::Unavailable);
    assert_eq!(rows[1].file_count, None);
    assert!(bindings.is_empty());
    let context = fixture.context();
    let binding = DirectoryBinding {
        candidate: context.candidates[1].clone(),
        canonical: std::fs::canonicalize(checks).unwrap(),
    };
    assert!(
        open_directory(
            Some(&binding),
            &context,
            || true,
            |_| panic!("Remote cache")
        )
        .is_err()
    );
}

#[test]
fn remote_cache_resolution_does_not_create_storage_and_fails_closed_without_an_existing_parent() {
    let fixture = Fixture::new();
    let cache = remote_cache_at(fixture.root.clone()).unwrap();
    assert_eq!(cache.path, fixture.remote);
    assert_eq!(cache.canonical, fixture.remote);
    assert!(!fixture.remote.exists());
    assert!(remote_cache_at(fixture.root.join("missing-data-directory")).is_err());
    assert!(!fixture.root.join("missing-data-directory").exists());
}

#[cfg(unix)]
#[test]
fn remote_cache_symlinks_cannot_turn_cached_paths_into_local_folder_authority() {
    let fixture = Fixture::new();
    let local = fixture.root.join("local-project");
    std::fs::create_dir_all(local.join(".antiburn/checks")).unwrap();
    std::fs::create_dir(&fixture.remote).unwrap();
    let cached = fixture.remote.join("cached-project");
    std::os::unix::fs::symlink(&local, &cached).unwrap();
    fixture
        .store
        .replace_repositories(&[repository(&cached)])
        .unwrap();
    let (rows, bindings) = fixture.scan();
    assert_eq!(rows[1].state, DirectoryState::Unavailable);
    assert!(bindings.is_empty());
    let context = fixture.context();
    let binding = DirectoryBinding {
        candidate: context.candidates[1].clone(),
        canonical: std::fs::canonicalize(local.join(".antiburn/checks")).unwrap(),
    };
    assert!(
        open_directory(
            Some(&binding),
            &context,
            || true,
            |_| panic!("Cached symlink path")
        )
        .is_err()
    );
}

#[test]
fn every_settled_explicit_open_records_exactly_one_closed_completion_and_safe_error() {
    let fixture = Fixture::new();
    fixture.create_global();
    let (rows, bindings) = fixture.scan();
    for (stage, expected) in [
        ("success", BurnCheckDirectoryOpenOutcome::Succeeded),
        ("opener", BurnCheckDirectoryOpenOutcome::OpenFailed),
        ("missing", BurnCheckDirectoryOpenOutcome::Missing),
        ("permission", BurnCheckDirectoryOpenOutcome::Unavailable),
        ("worker", BurnCheckDirectoryOpenOutcome::OpenFailed),
    ] {
        let mut opens = 0;
        let reference = rows[0].reference.as_ref().unwrap();
        let result = if stage == "worker" {
            Err(DirectoryOpenError::open_failed())
        } else {
            open_directory(
                if stage == "missing" {
                    None
                } else {
                    bindings.get(reference)
                },
                &fixture.context(),
                || stage != "permission",
                |_| {
                    opens += 1;
                    if stage == "opener" { Err(()) } else { Ok(()) }
                },
            )
        };
        let mut outcomes = Vec::new();
        let result = complete_open(result, |outcome| outcomes.push(outcome));
        assert_eq!(outcomes, [expected], "{stage}");
        assert_eq!(opens, usize::from(matches!(stage, "success" | "opener")));
        assert_eq!(result.is_ok(), stage == "success");
        if let Err(error) = result {
            assert!(!error.contains(reference));
            assert!(!error.contains(fixture.root.to_str().unwrap()));
            assert!(!error.contains("os error"));
        }
    }
}

#[test]
fn window_and_ipc_guards_allow_only_settings_or_main_with_one_reference_argument() {
    for label in [crate::settings::LABEL, crate::main_window::LABEL] {
        assert!(check_window(label).is_ok());
    }
    for label in ["popover", "hud", "unknown", "settings-child"] {
        assert!(check_window(label).is_err());
    }
    assert!(
        open_arguments(&tauri::ipc::InvokeBody::Json(
            serde_json::json!({"directoryReference": "issued"})
        ))
        .is_ok()
    );
    for body in [
        serde_json::json!({"path": "/private/folder"}),
        serde_json::json!({"directoryReference": "issued", "path": "/private/folder"}),
        serde_json::json!({"directoryReference": "issued", "program": "shell"}),
        serde_json::json!({"directory_reference": "issued"}),
    ] {
        assert!(open_arguments(&tauri::ipc::InvokeBody::Json(body)).is_err());
    }
    assert!(open_arguments(&tauri::ipc::InvokeBody::Raw(Vec::new())).is_err());
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../../../capabilities/checks-settings.json")).unwrap();
    assert_eq!(
        capability["windows"],
        serde_json::json!(["main", "settings"])
    );
    let commands = include_str!("../../app_commands.rs");
    for command in [
        "list_installed_check_directories",
        "open_installed_check_directory",
    ] {
        assert!(commands.contains(command));
        let permission = format!("allow-{}", command.replace('_', "-"));
        assert!(
            capability["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == &permission)
        );
        for broad in [
            include_str!("../../../capabilities/default.json"),
            include_str!("../../../capabilities/main.json"),
            include_str!("../../../permissions/default.toml"),
        ] {
            assert!(!broad.contains(&permission));
        }
    }
}

#[test]
fn directory_dto_has_only_the_shared_camel_case_fields() {
    let fixture = Fixture::new();
    fixture.create_global();
    let row = fixture.scan().0.remove(0);
    let value = serde_json::to_value(&row).unwrap();
    let keys: std::collections::BTreeSet<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "reference",
            "scope",
            "label",
            "path",
            "state",
            "fileCount",
            "message"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(value["scope"], "global");
    assert_eq!(value["state"], "ready");
    assert_eq!(value["fileCount"], 0);
}
