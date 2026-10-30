use std::collections::HashSet;

use super::*;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreparationStage {
    Identity,
    Filter,
    Title,
    Git,
    Fingerprint,
    FingerprintCommit,
}

#[cfg(test)]
pub(super) struct PreparationHooks {
    pub stage: PreparationStage,
    pub blocked: HashSet<String>,
    pub panic: HashSet<String>,
    pub panic_once: AtomicBool,
    pub entered: tokio::sync::mpsc::UnboundedSender<String>,
    pub release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl PreparationHooks {
    pub async fn wait(&self, log: &SessionLog, stage: PreparationStage) {
        if stage != self.stage {
            return;
        }
        let label = log.source_label();
        if self.blocked.contains(&label) {
            self.entered
                .send(label.clone())
                .expect("barrier receiver stays open");
            self.release
                .acquire()
                .await
                .expect("barrier stays open")
                .forget();
        }
        if self.panic.contains(&label) && self.panic_once.swap(false, Ordering::SeqCst) {
            panic!("synthetic preparation panic");
        }
    }
}

#[derive(Debug)]
pub(super) struct PreparedDescription {
    pub(super) log: SessionLog,
    pub(super) previous: Option<SessionRecord>,
    pub(super) outcome: PreparationOutcome,
    pub(super) gate: GateCounts,
}

#[derive(Debug)]
pub(super) enum PreparationOutcome {
    Accepted {
        record: Box<SessionRecord>,
        changed: bool,
    },
    Rejected(SessionKey),
    OutsideWindow,
    Deferred(String),
    Skipped,
}

/// Prepare one candidate under the caller's fixed cutoff and policy snapshot.
#[cfg(test)]
pub(super) async fn prepare_description(
    log: SessionLog,
    home: PathBuf,
    ignored: HashSet<String>,
    previous: Option<SessionRecord>,
    cutoff: Option<i64>,
    include_non_repo_folders: bool,
) -> PreparedDescription {
    let descriptions = prepare_description_logs(
        vec![log.clone()],
        &previous
            .iter()
            .map(|record| {
                (
                    SessionActivityKey::new(
                        record.key.environment_key.clone(),
                        record.key.agent.clone(),
                        record.source_label.clone(),
                    ),
                    record.clone(),
                )
            })
            .collect(),
        false,
    )
    .await
    .expect("source identity preparation has no store writes");
    let Some(description) = descriptions.into_iter().next() else {
        return PreparedDescription {
            log,
            previous,
            outcome: PreparationOutcome::Skipped,
            gate: GateCounts::default(),
        };
    };
    prepare_description_with_read(
        description,
        home,
        ignored,
        previous,
        cutoff,
        include_non_repo_folders,
        None,
    )
    .await
}

pub(super) async fn prepare_description_with_read(
    description: DescriptionLog,
    home: PathBuf,
    ignored: HashSet<String>,
    previous: Option<SessionRecord>,
    cutoff: Option<i64>,
    include_non_repo_folders: bool,
    #[cfg(test)] hooks: Option<Arc<PreparationHooks>>,
) -> PreparedDescription {
    let log = description.log.clone();
    #[cfg(test)]
    if let Some(hooks) = &hooks {
        hooks.wait(&log, PreparationStage::Filter).await;
    }
    let mut gate = GateCounts::default();
    let activity = if let Some(cutoff) = cutoff {
        let children = subagent_children_for(&log).await;
        let (updated_at_epoch, activity_source, activity_cursor) = semantic_activity_for_log(
            &log,
            previous.as_ref(),
            &children,
            description
                .read
                .as_ref()
                .and_then(|read| read.content.as_deref()),
        )
        .await;
        if activity_source != "unknown" && updated_at_epoch.is_some_and(|at| at < cutoff) {
            return PreparedDescription {
                log,
                previous,
                outcome: PreparationOutcome::OutsideWindow,
                gate,
            };
        }
        Some(CandidateActivity {
            updated_at_epoch,
            activity_source,
            activity_cursor,
        })
    } else {
        None
    };
    #[cfg(test)]
    if let Some(hooks) = &hooks {
        hooks.wait(&log, PreparationStage::Title).await;
    }
    let indexed_title = indexed_title_for_candidate(&log, &description.key.session_id).await;
    let (_, described, changed) = describe_candidate_with_read(
        description,
        home,
        previous.clone(),
        activity,
        indexed_title,
        Instant::now(),
    )
    .await;
    let outcome = match described {
        DescribeOutcome::Session(record) => {
            #[cfg(test)]
            if let Some(hooks) = &hooks {
                hooks.wait(&log, PreparationStage::Git).await;
            }
            admit_record(
                record,
                changed,
                &ignored,
                include_non_repo_folders,
                &mut gate,
            )
            .await
        }
        DescribeOutcome::Subagent(key) => {
            gate.subagent += 1;
            PreparationOutcome::Rejected(key)
        }
        DescribeOutcome::Skip => PreparationOutcome::Skipped,
    };
    PreparedDescription {
        log,
        previous,
        outcome,
        gate,
    }
}

async fn indexed_title_for_candidate(log: &SessionLog, session_id: &str) -> Option<ResolvedTitle> {
    if !should_batch_indexed_title(log) {
        return None;
    }
    Explorers::DISK
        .get(&log.agent_type)
        .indexed_session_title(session_id)
        .await
}

async fn admit_record(
    mut record: Box<SessionRecord>,
    mut changed: bool,
    ignored: &HashSet<String>,
    include_non_repo_folders: bool,
    gate: &mut GateCounts,
) -> PreparationOutcome {
    let Some(cwd) = record.cwd.clone() else {
        gate.missing_cwd += 1;
        return PreparationOutcome::Rejected(record.key);
    };
    if ignored_paths::set_contains(ignored, &cwd) {
        gate.ignored += 1;
        return PreparationOutcome::Rejected(record.key);
    }
    let admission = repo_admission(&record, &cwd, include_non_repo_folders).await;
    let root = match apply_prepared_admission(admission, &mut record, &mut changed, gate) {
        ControlFlow::Continue(root) => root,
        ControlFlow::Break(outcome) => return outcome,
    };
    if let Some(root) = root {
        let root = git::canonical_main_repo_root(&root).await;
        if ignored_paths::is_session_ignored(
            ignored,
            record.cwd.as_deref().or(Some(&cwd)),
            &root.to_string_lossy(),
        ) {
            gate.ignored += 1;
            return PreparationOutcome::Rejected(record.key);
        }
    }
    PreparationOutcome::Accepted { record, changed }
}

fn apply_prepared_admission(
    admission: RepoAdmission,
    record: &mut SessionRecord,
    changed: &mut bool,
    gate: &mut GateCounts,
) -> ControlFlow<PreparationOutcome, Option<PathBuf>> {
    let mut rejected = Vec::new();
    let mut git_hold = GitHold::default();
    match apply_repo_admission(
        admission,
        record,
        changed,
        gate,
        &mut rejected,
        &mut git_hold,
    ) {
        ControlFlow::Continue(root) => ControlFlow::Continue(root),
        ControlFlow::Break(()) => ControlFlow::Break(match git_hold.error {
            Some(error) => PreparationOutcome::Deferred(error),
            None => PreparationOutcome::Rejected(
                rejected
                    .pop()
                    .expect("repository rejection supplies a session key"),
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::process::Command;

    const SESSION_ID: &str = "preparation-fixture";
    const EVENT_TIME: &str = "2026-08-01T10:01:00Z";

    struct Fixture {
        home: tempfile::TempDir,
        cwd: PathBuf,
        log: SessionLog,
    }

    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir().expect("create fixture home");
            let root = home.path().canonicalize().expect("canonical fixture home");
            let cwd = root.join("workspace");
            std::fs::create_dir(&cwd).expect("create session folder");
            let project = root.join(".claude/projects/-synthetic-workspace");
            std::fs::create_dir_all(&project).expect("create Claude project folder");
            let fixture = Self {
                home,
                cwd,
                log: SessionLog {
                    agent_type: AgentKind::Claude,
                    source: SessionSource::File(project.join(format!("{SESSION_ID}.jsonl"))),
                    updated_at: Some(event_epoch() + 60 * 86_400),
                    environment: DiscoveryEnvironment::Native,
                },
            };
            fixture.write_events(&[
                fixture.user_event(),
                json!({
                    "type": "custom-title",
                    "customTitle": "Repair the synthetic build",
                    "timestamp": "2026-10-01T10:01:00Z",
                }),
            ]);
            fixture
        }

        fn user_event(&self) -> Value {
            json!({
                "type": "user",
                "sessionId": SESSION_ID,
                "cwd": self.cwd,
                "timestamp": EVENT_TIME,
                "message": {"role": "user", "content": "Repair the synthetic build"},
            })
        }

        fn write_events(&self, events: &[Value]) {
            let SessionSource::File(path) = &self.log.source else {
                panic!("fixture has a file source");
            };
            let content = events
                .iter()
                .map(|event| format!("{event}\n"))
                .collect::<String>();
            std::fs::write(path, content).expect("write synthetic Claude transcript");
        }

        fn home(&self) -> PathBuf {
            self.home.path().to_path_buf()
        }
    }

    fn event_epoch() -> i64 {
        time::OffsetDateTime::parse(EVENT_TIME, &time::format_description::well_known::Rfc3339)
            .expect("parse fixture timestamp")
            .unix_timestamp()
    }

    fn git_command(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run fixture Git command");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn accepted(prepared: PreparedDescription) -> (Box<SessionRecord>, bool) {
        match prepared.outcome {
            PreparationOutcome::Accepted { record, changed } => (record, changed),
            outcome => panic!("expected an accepted session, got {outcome:?}"),
        }
    }

    fn assert_rejected(prepared: &PreparedDescription) {
        assert!(
            matches!(
                &prepared.outcome,
                PreparationOutcome::Rejected(key)
                    if key == &SessionKey::new("native", AgentKind::Claude.slug(), SESSION_ID)
            ),
            "expected fixture rejection, got {:?}",
            prepared.outcome
        );
    }

    #[tokio::test]
    async fn production_preparation_accepts_a_repository_and_reuses_its_record() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let first = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            Some(event_epoch()),
            false,
        )
        .await;
        assert_eq!(first.log.source_label(), fixture.log.source_label());
        assert!(first.previous.is_none());
        assert_eq!(first.gate, GateCounts::default());
        let (record, changed) = accepted(first);
        assert!(changed);
        assert_eq!(
            record.key,
            SessionKey::new("native", AgentKind::Claude.slug(), SESSION_ID)
        );
        assert_eq!(record.cwd.as_deref(), fixture.cwd.to_str());
        assert_eq!(record.title.as_deref(), Some("Repair the synthetic build"));
        assert_eq!(record.updated_at_epoch, Some(event_epoch()));
        assert_eq!(record.activity_source, "event");
        assert!(!record.activity_cursor.is_empty());
        assert!(record.source_fingerprint.is_some());

        let previous = *record;
        let second = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            Some(previous.clone()),
            Some(event_epoch()),
            false,
        )
        .await;
        assert_eq!(second.previous.as_ref(), Some(&previous));
        let (reused, changed) = accepted(second);
        assert!(!changed);
        assert_eq!(*reused, previous);
    }

    #[tokio::test]
    async fn production_preparation_uses_semantic_activity_for_the_fixed_cutoff() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let initial = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            false,
        )
        .await;
        let previous = *accepted(initial).0;
        let outside = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            Some(previous.clone()),
            Some(event_epoch() + 1),
            false,
        )
        .await;
        assert!(matches!(outside.outcome, PreparationOutcome::OutsideWindow));
        assert_eq!(outside.previous, Some(previous));
        assert_eq!(outside.gate, GateCounts::default());

        let cold = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            Some(event_epoch() + 1),
            false,
        )
        .await;
        assert!(matches!(cold.outcome, PreparationOutcome::OutsideWindow));
    }

    #[tokio::test]
    async fn production_preparation_includes_child_activity_in_the_cutoff() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let SessionSource::File(parent) = &fixture.log.source else {
            panic!("fixture has a file source");
        };
        let children = parent.parent().unwrap().join(SESSION_ID).join("subagents");
        std::fs::create_dir_all(&children).expect("create subagent folder");
        std::fs::write(
            children.join("agent-child.jsonl"),
            concat!(
                r#"{"type":"assistant","timestamp":"2026-08-01T10:02:00Z"}"#,
                "\n",
            ),
        )
        .expect("write child transcript");

        let prepared = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            Some(event_epoch() + 60),
            false,
        )
        .await;
        let (record, _) = accepted(prepared);
        assert_eq!(record.updated_at_epoch, Some(event_epoch() + 60));
        assert_eq!(record.subagent_count, 1);
    }

    #[tokio::test]
    async fn production_preparation_rechecks_ignored_cwd_for_an_unchanged_record() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let initial = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            false,
        )
        .await;
        let previous = *accepted(initial).0;
        let ignored = HashSet::from([fixture.cwd.to_string_lossy().into_owned()]);
        let prepared = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            ignored,
            Some(previous.clone()),
            None,
            true,
        )
        .await;
        assert_rejected(&prepared);
        assert_eq!(prepared.previous, Some(previous));
        assert_eq!(
            prepared.gate,
            GateCounts {
                ignored: 1,
                ..GateCounts::default()
            }
        );
    }

    #[tokio::test]
    async fn production_preparation_enforces_the_folder_setting_for_cached_records() {
        let fixture = Fixture::new();
        let excluded = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            false,
        )
        .await;
        assert_rejected(&excluded);
        assert_eq!(
            excluded.gate,
            GateCounts {
                no_repo: 1,
                ..GateCounts::default()
            }
        );

        let included = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            true,
        )
        .await;
        assert_eq!(
            included.gate,
            GateCounts {
                folder: 1,
                ..GateCounts::default()
            }
        );
        let previous = *accepted(included).0;
        let excluded_again = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            Some(previous),
            None,
            false,
        )
        .await;
        assert_rejected(&excluded_again);
        assert_eq!(excluded_again.gate.no_repo, 1);
    }

    #[tokio::test]
    async fn production_preparation_rejects_subagents_and_missing_cwd() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let mut sidechain = fixture.user_event();
        sidechain["isSidechain"] = json!(true);
        fixture.write_events(&[sidechain]);
        let subagent = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            true,
        )
        .await;
        assert_rejected(&subagent);
        assert_eq!(
            subagent.gate,
            GateCounts {
                subagent: 1,
                ..GateCounts::default()
            }
        );

        let mut missing_cwd = fixture.user_event();
        missing_cwd.as_object_mut().unwrap().remove("cwd");
        fixture.write_events(&[missing_cwd]);
        let missing = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            true,
        )
        .await;
        assert_rejected(&missing);
        assert_eq!(
            missing.gate,
            GateCounts {
                missing_cwd: 1,
                ..GateCounts::default()
            }
        );
    }

    #[tokio::test]
    async fn production_preparation_infers_and_checks_a_repository_below_cwd() {
        let fixture = Fixture::new();
        let repository = fixture.cwd.join("app");
        std::fs::create_dir(&repository).expect("create nested repository");
        git_command(&repository, &["init", "-q"]);
        fixture.write_events(&[
            fixture.user_event(),
            json!({"input": {"file_path": repository.join("README.md")}}),
        ]);
        let prepared = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            false,
        )
        .await;
        let (record, changed) = accepted(prepared);
        assert!(changed);
        assert_eq!(record.cwd.as_deref(), repository.to_str());

        let ignored = HashSet::from([repository.to_string_lossy().into_owned()]);
        let excluded = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            ignored,
            None,
            None,
            true,
        )
        .await;
        assert_rejected(&excluded);
        assert_eq!(
            excluded.gate,
            GateCounts {
                ignored: 1,
                ..GateCounts::default()
            }
        );
    }

    #[tokio::test]
    async fn production_preparation_checks_the_main_repository_for_linked_worktrees() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        git_command(
            &fixture.cwd,
            &[
                "-c",
                "user.name=Synthetic Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        let worktree = fixture.home().join("linked-worktree");
        git_command(
            &fixture.cwd,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        );
        let mut event = fixture.user_event();
        event["cwd"] = json!(worktree);
        fixture.write_events(&[event]);
        let included = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            false,
        )
        .await;
        let (record, _) = accepted(included);
        assert_eq!(record.cwd.as_deref(), worktree.to_str());

        let ignored = HashSet::from([fixture.cwd.to_string_lossy().into_owned()]);
        let excluded = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            ignored,
            None,
            None,
            false,
        )
        .await;
        assert_rejected(&excluded);
        assert_eq!(
            excluded.gate,
            GateCounts {
                ignored: 1,
                ..GateCounts::default()
            }
        );
    }

    #[tokio::test]
    async fn production_preparation_keeps_unknown_activity_and_skips_empty_identity() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let mut log = fixture.log.clone();
        log.source = SessionSource::Inline {
            label: SESSION_ID.into(),
            content: format!("{}\n", fixture.user_event()),
            source_format: None,
        };
        let prepared = prepare_description(
            log,
            fixture.home(),
            HashSet::new(),
            None,
            Some(i64::MAX),
            false,
        )
        .await;
        assert!(matches!(
            prepared.outcome,
            PreparationOutcome::Accepted { .. }
        ));

        let mut log = fixture.log.clone();
        log.source = SessionSource::Inline {
            label: String::new(),
            content: String::new(),
            source_format: None,
        };
        let skipped =
            prepare_description(log, fixture.home(), HashSet::new(), None, None, false).await;
        assert!(matches!(skipped.outcome, PreparationOutcome::Skipped));
        assert_eq!(skipped.gate, GateCounts::default());
    }

    #[tokio::test]
    async fn a_git_outage_defers_a_prepared_record_without_rejecting_it() {
        let fixture = Fixture::new();
        git_command(&fixture.cwd, &["init", "-q"]);
        let prepared = prepare_description(
            fixture.log.clone(),
            fixture.home(),
            HashSet::new(),
            None,
            None,
            false,
        )
        .await;
        let (mut record, mut changed) = accepted(prepared);
        let mut gate = GateCounts::default();
        let result = apply_prepared_admission(
            RepoAdmission::GitUnavailable("synthetic Git outage".into()),
            &mut record,
            &mut changed,
            &mut gate,
        );
        assert!(matches!(result,
            ControlFlow::Break(PreparationOutcome::Deferred(error)) if error == "synthetic Git outage"
        ));
        assert_eq!(gate, GateCounts::default());
    }
}
