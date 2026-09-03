//! Scoped passes: the narrow work a watcher burst can answer without a full
//! discovery walk. See `docs/plans/continuous-session-ingest.md`, "Phase 5b",
//! rules T1 to T7, for the contract this module implements.
//!
//! [`classify_burst`] sorts a burst's paths into a [`ScopedWork`]: known
//! sessions to refresh directly (T1), agents to rediscover because a new or
//! database-backed session appeared under their root (T3, T5), or paths
//! nothing claims (T6). [`Floors`] then admits the part of that work that has
//! not run too recently, per session and per agent (T2, T4, T5), and defers
//! the rest without dropping it (T7).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use antiburn_local::discovery::{Explorers, WatchRoot};
use antiburn_local::model::AgentKind;
use tokio::time::Instant;

use crate::store::SessionActivityKey;

/// T2: a re-described session is not re-described again before this many
/// seconds pass.
pub const TARGETED_MIN_INTERVAL: Duration = Duration::from_secs(10);

/// T4: an agent's rediscovery runs at most this often.
pub const AGENT_REDISCOVER_MIN_INTERVAL: Duration = Duration::from_secs(20);

/// T5: a database-backed agent's rediscovery runs at most this often — longer
/// than [`AGENT_REDISCOVER_MIN_INTERVAL`] because every write under such an
/// agent's root looks like a new session to the classifier.
pub const DB_AGENT_REDISCOVER_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// File name suffixes the classifier treats as a database-backed agent's
/// store (T5): the vendor extensions themselves, plus their SQLite WAL and
/// rollback-journal sidecars.
const DATABASE_EXTENSIONS: [&str; 4] = ["vscdb", "db", "sqlite", "sqlite3"];
const DATABASE_SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-journal"];

/// What one path in a burst means for the scheduler, in the classification
/// order T1, T5, T3, T6 apply.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClassifiedPath {
    Session(SessionActivityKey),
    DatabaseAgent(AgentKind),
    Agent(AgentKind),
    Ignored,
}

/// A burst's classifications, folded together. Sets rather than counts: the
/// scheduler merges bursts across wakes (T7), and a repeated path or agent
/// must not be double-counted or double-admitted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScopedWork {
    pub sessions: BTreeSet<SessionActivityKey>,
    pub agents: BTreeSet<AgentKind>,
    pub db_agents: BTreeSet<AgentKind>,
}

impl ScopedWork {
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.agents.is_empty() && self.db_agents.is_empty()
    }

    /// Fold another burst's work in, dropping nothing (T7).
    pub fn merge(&mut self, other: ScopedWork) {
        self.sessions.extend(other.sessions);
        self.agents.extend(other.agents);
        self.db_agents.extend(other.db_agents);
    }
}

/// Classify every path in a burst against `home` and `lookup`, and fold the
/// result into one [`ScopedWork`]. Pure over its arguments — no I/O beyond
/// what `lookup` does — so a test can drive it with a fake lookup and a
/// synthetic home.
///
/// `lookup` answers "is this string form a stored native file session's
/// `source_label`", the store query [`crate::store::Store::session_record_by_source_label`]
/// backs in production.
pub fn classify_burst(
    paths: &[PathBuf],
    home: &Path,
    lookup: &dyn Fn(&str) -> Option<SessionActivityKey>,
) -> ScopedWork {
    let roots = all_agent_roots(home);
    let mut work = ScopedWork::default();
    let mut ignored = 0usize;
    for path in paths {
        match classify_path(path, &roots, lookup) {
            ClassifiedPath::Session(key) => {
                work.sessions.insert(key);
            }
            ClassifiedPath::DatabaseAgent(agent) => {
                work.db_agents.insert(agent);
            }
            ClassifiedPath::Agent(agent) => {
                work.agents.insert(agent);
            }
            ClassifiedPath::Ignored => ignored += 1,
        }
    }
    // An agent already admitted on its own file evidence needs no separate
    // database-triggered rediscovery: both lead to the same
    // `discover_recent` call, and the shorter T3 floor already covers it.
    work.db_agents.retain(|agent| !work.agents.contains(agent));
    ::tracing::debug!(
        event = "scan_burst_classified",
        sessions = work.sessions.len(),
        agents = work.agents.len(),
        db_agents = work.db_agents.len(),
        ignored,
    );
    work
}

fn classify_path(
    path: &Path,
    roots: &[(AgentKind, WatchRoot)],
    lookup: &dyn Fn(&str) -> Option<SessionActivityKey>,
) -> ClassifiedPath {
    if let Some(key) = lookup(&path.to_string_lossy()) {
        return ClassifiedPath::Session(key);
    }
    // T1's sub-agent form: `<dir>/<sessionId>/subagents/agent-*.jsonl` names
    // its parent transcript as `<dir>/<sessionId>.jsonl`. Walk ancestors
    // while they stay under an agent's watch root, stopping at the first
    // ancestor whose `.jsonl` sibling is a stored session.
    let mut current = path.parent();
    while let Some(dir) = current {
        if !roots.iter().any(|(_, root)| dir.starts_with(&root.path)) {
            break;
        }
        let candidate = dir.with_extension("jsonl");
        if let Some(key) = lookup(&candidate.to_string_lossy()) {
            return ClassifiedPath::Session(key);
        }
        current = dir.parent();
    }
    if is_database_path(path)
        && let Some(agent) = owning_agent(path, roots)
    {
        return ClassifiedPath::DatabaseAgent(agent);
    }
    match owning_agent(path, roots) {
        Some(agent) => ClassifiedPath::Agent(agent),
        None => ClassifiedPath::Ignored,
    }
}

/// T5: whether `path`'s file name is one of the database extensions, or a
/// `-wal` / `-journal` sidecar of one.
fn is_database_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str())
        && DATABASE_EXTENSIONS
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    {
        return true;
    }
    DATABASE_SIDECAR_SUFFIXES.iter().any(|suffix| {
        name.strip_suffix(suffix).is_some_and(|stem| {
            Path::new(stem)
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    DATABASE_EXTENSIONS
                        .iter()
                        .any(|candidate| ext.eq_ignore_ascii_case(candidate))
                })
        })
    })
}

/// The agent whose watch root is the longest matching prefix of `path`, or
/// `None` when no watch root claims it.
///
/// T3: watch-root prefix matching, not [`Explorers::infer_agent_and_surface`]
/// — the watch roots are exactly what the watcher registered, so this
/// classification cannot disagree with what was watched.
pub fn owning_agent(path: &Path, roots: &[(AgentKind, WatchRoot)]) -> Option<AgentKind> {
    roots
        .iter()
        .filter(|(_, root)| path.starts_with(&root.path))
        .max_by_key(|(_, root)| root.path.as_os_str().len())
        .map(|(agent, _)| *agent)
}

/// Every agent's watch roots, resolved against `home`, paired with the agent
/// that owns each — the same roots the watcher registered, built fresh per
/// classification since a burst is rare enough that caching is not worth the
/// staleness risk of an agent installed after start-up.
fn all_agent_roots(home: &Path) -> Vec<(AgentKind, WatchRoot)> {
    AgentKind::ALL
        .iter()
        .flat_map(|agent| {
            Explorers::DISK
                .watch_roots_for(agent, home)
                .into_iter()
                .map(move |root| (*agent, root))
        })
        .collect()
}

/// Tracks the last admitted run per session and per agent, so the scheduler
/// can enforce T2, T4, and T5's floors without dropping the work a floor
/// defers (T7).
#[derive(Debug, Default)]
pub struct Floors {
    sessions: HashMap<SessionActivityKey, Instant>,
    agents: HashMap<AgentKind, Instant>,
}

impl Floors {
    /// Split `work` into the part whose floor has elapsed (admitted, and
    /// stamped here as run at `now`) and the part still inside its floor
    /// (deferred, unstamped). Returns the deferred part's earliest due
    /// instant, for the scheduler's sleep arm.
    pub fn admit(
        &mut self,
        work: ScopedWork,
        now: Instant,
    ) -> (ScopedWork, ScopedWork, Option<Instant>) {
        let mut run_now = ScopedWork::default();
        let mut deferred = ScopedWork::default();
        let mut earliest_due = None;

        for key in work.sessions {
            let last_run = self.sessions.get(&key).copied();
            let mut outcome = Admission {
                run_now: &mut run_now.sessions,
                deferred: &mut deferred.sessions,
                earliest_due: &mut earliest_due,
            };
            outcome.admit(key, last_run, TARGETED_MIN_INTERVAL, now);
        }
        for agent in work.agents {
            let last_run = self.agents.get(&agent).copied();
            let mut outcome = Admission {
                run_now: &mut run_now.agents,
                deferred: &mut deferred.agents,
                earliest_due: &mut earliest_due,
            };
            outcome.admit(agent, last_run, AGENT_REDISCOVER_MIN_INTERVAL, now);
        }
        for agent in work.db_agents {
            let last_run = self.agents.get(&agent).copied();
            let mut outcome = Admission {
                run_now: &mut run_now.db_agents,
                deferred: &mut deferred.db_agents,
                earliest_due: &mut earliest_due,
            };
            outcome.admit(agent, last_run, DB_AGENT_REDISCOVER_MIN_INTERVAL, now);
        }

        self.stamp(&run_now, now);
        (run_now, deferred, earliest_due)
    }

    /// Stamp every item in `work` as run at `now`, without deciding anything.
    /// A full pass re-describes everything a scoped pass would have, so it
    /// supersedes and satisfies every floor for the work it displaced.
    pub fn stamp(&mut self, work: &ScopedWork, now: Instant) {
        for key in &work.sessions {
            self.sessions.insert(key.clone(), now);
        }
        for agent in work.agents.iter().chain(work.db_agents.iter()) {
            self.agents.insert(*agent, now);
        }
    }
}

/// One floor's decision output, bundled so [`Floors::admit`] can hand it to
/// [`Admission::admit`] as a single argument per item kind.
struct Admission<'a, K: Ord> {
    run_now: &'a mut BTreeSet<K>,
    deferred: &'a mut BTreeSet<K>,
    earliest_due: &'a mut Option<Instant>,
}

impl<K: Ord> Admission<'_, K> {
    /// Admit `item` now when its floor has elapsed since `last_run`, or
    /// defer it and widen `earliest_due` to cover when it will.
    fn admit(&mut self, item: K, last_run: Option<Instant>, floor: Duration, now: Instant) {
        match last_run.map(|last| last + floor) {
            Some(due) if due > now => {
                self.deferred.insert(item);
                *self.earliest_due =
                    Some(self.earliest_due.map_or(due, |current| current.min(due)));
            }
            _ => {
                self.run_now.insert(item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str, recursive: bool) -> WatchRoot {
        let path = PathBuf::from(path);
        if recursive {
            WatchRoot::recursive(path)
        } else {
            WatchRoot::shallow(path)
        }
    }

    fn key(source_label: &str) -> SessionActivityKey {
        SessionActivityKey::new("native", "claude-code", source_label)
    }

    /// Cursor's real watch root for `home`, so a `classify_burst` test (which
    /// resolves roots through the actual [`Explorers::DISK`] registry, not a
    /// fake) exercises a path the classifier would really see — the root's
    /// shape differs by platform.
    fn cursor_watch_root(home: &Path) -> PathBuf {
        Explorers::DISK
            .watch_roots_for(&AgentKind::Cursor, home)
            .into_iter()
            .next()
            .expect("Cursor has at least one watch root")
            .path
    }

    #[test]
    fn a_path_equal_to_a_stored_source_label_is_a_known_session() {
        let home = PathBuf::from("/home/avery");
        let session_path = home.join(".claude/projects/demo/abc.jsonl");
        let expected = key(&session_path.to_string_lossy());
        let lookup_key = expected.clone();
        let lookup =
            move |label: &str| (label == lookup_key.source_label).then(|| lookup_key.clone());

        let work = classify_burst(&[session_path], &home, &lookup);
        assert_eq!(work.sessions, BTreeSet::from([expected]));
        assert!(work.agents.is_empty());
        assert!(work.db_agents.is_empty());
    }

    #[test]
    fn a_subagent_transcript_resolves_to_its_parent_sessions_jsonl_sibling() {
        let home = PathBuf::from("/home/avery");
        let parent = home.join(".claude/projects/demo/abc.jsonl");
        let subagent = home.join(".claude/projects/demo/abc/subagents/agent-1.jsonl");
        let expected = key(&parent.to_string_lossy());
        let lookup_key = expected.clone();
        let lookup =
            move |label: &str| (label == lookup_key.source_label).then(|| lookup_key.clone());

        let work = classify_burst(&[subagent], &home, &lookup);
        assert_eq!(work.sessions, BTreeSet::from([expected]));
    }

    #[test]
    fn a_database_path_under_an_agents_root_is_a_database_agent() {
        let home = PathBuf::from("/home/avery");
        let db_path = home.join(".cursor/state.vscdb");
        let roots = [(AgentKind::Cursor, root("/home/avery/.cursor", true))];
        let lookup = |_: &str| None;

        assert_eq!(
            classify_path(&db_path, &roots, &lookup),
            ClassifiedPath::DatabaseAgent(AgentKind::Cursor)
        );
        let wal = home.join(".cursor/state.vscdb-wal");
        assert_eq!(
            classify_path(&wal, &roots, &lookup),
            ClassifiedPath::DatabaseAgent(AgentKind::Cursor)
        );
        let journal = home.join(".cursor/state.vscdb-journal");
        assert_eq!(
            classify_path(&journal, &roots, &lookup),
            ClassifiedPath::DatabaseAgent(AgentKind::Cursor)
        );
    }

    #[test]
    fn a_non_database_path_under_an_agents_root_is_a_plain_agent() {
        let home = PathBuf::from("/home/avery");
        let path = home.join(".codex/sessions/2026/08/01/rollout.jsonl");
        let roots = [(AgentKind::Codex, root("/home/avery/.codex", true))];
        let lookup = |_: &str| None;

        assert_eq!(
            classify_path(&path, &roots, &lookup),
            ClassifiedPath::Agent(AgentKind::Codex)
        );
    }

    #[test]
    fn an_unclaimed_path_is_ignored() {
        let path = PathBuf::from("/home/avery/Downloads/random.txt");
        let roots: [(AgentKind, WatchRoot); 0] = [];
        let lookup = |_: &str| None;

        assert_eq!(
            classify_path(&path, &roots, &lookup),
            ClassifiedPath::Ignored
        );
    }

    #[test]
    fn a_burst_folds_into_all_three_buckets_and_counts_ignored() {
        let home = PathBuf::from("/home/avery");
        let session_path = home.join(".claude/projects/demo/known.jsonl");
        let db_path = cursor_watch_root(&home).join("state.vscdb");
        let agent_path = home.join(".codex/sessions/2026/08/01/rollout.jsonl");
        let unclaimed = PathBuf::from("/home/avery/Downloads/random.txt");

        let expected_key = key(&session_path.to_string_lossy());
        let lookup_key = expected_key.clone();
        let lookup =
            move |label: &str| (label == lookup_key.source_label).then(|| lookup_key.clone());

        let work = classify_burst(
            &[session_path, db_path, agent_path, unclaimed],
            &home,
            &lookup,
        );
        assert_eq!(work.sessions, BTreeSet::from([expected_key]));
        assert_eq!(work.agents, BTreeSet::from([AgentKind::Codex]));
        assert_eq!(work.db_agents, BTreeSet::from([AgentKind::Cursor]));
    }

    #[test]
    fn an_agent_with_both_plain_and_database_evidence_keeps_only_the_plain_classification() {
        let home = PathBuf::from("/home/avery");
        let cursor_root = cursor_watch_root(&home);
        let db_path = cursor_root.join("state.vscdb");
        let plain_path = cursor_root.join("some-other-file.json");
        let lookup = |_: &str| None;

        let work = classify_burst(&[db_path, plain_path], &home, &lookup);
        assert_eq!(work.agents, BTreeSet::from([AgentKind::Cursor]));
        assert!(work.db_agents.is_empty());
    }

    #[test]
    fn owning_agent_picks_the_longest_matching_root() {
        let roots = [
            (AgentKind::Cline, root("/home/avery/.config/Code", true)),
            (
                AgentKind::Cline,
                root("/home/avery/.config/Code/User/globalStorage/cline", true),
            ),
        ];
        let path =
            PathBuf::from("/home/avery/.config/Code/User/globalStorage/cline/tasks/1/api.json");
        assert_eq!(owning_agent(&path, &roots), Some(AgentKind::Cline));

        let outside = PathBuf::from("/home/avery/.config/Code/User/settings.json");
        assert_eq!(owning_agent(&outside, &roots), Some(AgentKind::Cline));

        let unrelated = PathBuf::from("/home/avery/.codex/sessions/x.jsonl");
        assert_eq!(owning_agent(&unrelated, &roots), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_is_deferred_inside_its_floor_and_admitted_after() {
        let mut floors = Floors::default();
        let session_key = key("/home/avery/.claude/projects/demo/x.jsonl");
        let mut work = ScopedWork::default();
        work.sessions.insert(session_key.clone());

        let t0 = Instant::now();
        let (run_now, deferred, _due) = floors.admit(work.clone(), t0);
        assert_eq!(run_now.sessions, BTreeSet::from([session_key.clone()]));
        assert!(deferred.sessions.is_empty());

        tokio::time::advance(Duration::from_secs(5)).await;
        let (run_now, deferred, due) = floors.admit(work.clone(), Instant::now());
        assert!(run_now.sessions.is_empty(), "still inside the 10s floor");
        assert_eq!(deferred.sessions, BTreeSet::from([session_key.clone()]));
        assert_eq!(due, Some(t0 + TARGETED_MIN_INTERVAL));

        tokio::time::advance(Duration::from_secs(5)).await;
        let (run_now, deferred, _due) = floors.admit(work, Instant::now());
        assert_eq!(run_now.sessions, BTreeSet::from([session_key]));
        assert!(deferred.sessions.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_agent_uses_the_twenty_second_floor() {
        let mut floors = Floors::default();
        let mut work = ScopedWork::default();
        work.agents.insert(AgentKind::Codex);

        let t0 = Instant::now();
        floors.admit(work.clone(), t0);

        tokio::time::advance(AGENT_REDISCOVER_MIN_INTERVAL - Duration::from_secs(1)).await;
        let (run_now, deferred, _due) = floors.admit(work.clone(), Instant::now());
        assert!(run_now.agents.is_empty());
        assert_eq!(deferred.agents, BTreeSet::from([AgentKind::Codex]));

        tokio::time::advance(Duration::from_secs(1)).await;
        let (run_now, _deferred, _due) = floors.admit(work, Instant::now());
        assert_eq!(run_now.agents, BTreeSet::from([AgentKind::Codex]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_database_agent_uses_the_thirty_second_floor() {
        let mut floors = Floors::default();
        let mut work = ScopedWork::default();
        work.db_agents.insert(AgentKind::Cursor);

        let t0 = Instant::now();
        floors.admit(work.clone(), t0);

        tokio::time::advance(AGENT_REDISCOVER_MIN_INTERVAL).await;
        let (run_now, deferred, _due) = floors.admit(work.clone(), Instant::now());
        assert!(
            run_now.db_agents.is_empty(),
            "the 20s agent floor must not admit a database agent early"
        );
        assert_eq!(deferred.db_agents, BTreeSet::from([AgentKind::Cursor]));

        tokio::time::advance(DB_AGENT_REDISCOVER_MIN_INTERVAL - AGENT_REDISCOVER_MIN_INTERVAL)
            .await;
        let (run_now, _deferred, _due) = floors.admit(work, Instant::now());
        assert_eq!(run_now.db_agents, BTreeSet::from([AgentKind::Cursor]));
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_work_merges_with_newly_classified_work() {
        let mut floors = Floors::default();
        let session_a = key("/home/avery/.claude/projects/demo/a.jsonl");
        let session_b = key("/home/avery/.claude/projects/demo/b.jsonl");

        let mut first = ScopedWork::default();
        first.sessions.insert(session_a.clone());
        let t0 = Instant::now();
        floors.admit(first.clone(), t0);

        // A second burst for the same session inside its floor is deferred,
        // not dropped.
        tokio::time::advance(Duration::from_secs(1)).await;
        let (run_now, deferred_a, _due) = floors.admit(first, Instant::now());
        assert!(run_now.sessions.is_empty(), "still inside the 10s floor");
        assert_eq!(deferred_a.sessions, BTreeSet::from([session_a.clone()]));

        // The deferred session merges with a newly classified one.
        let mut pending = ScopedWork::default();
        pending.merge(deferred_a);
        let mut second = ScopedWork::default();
        second.sessions.insert(session_b.clone());
        pending.merge(second);

        assert_eq!(
            pending.sessions,
            BTreeSet::from([session_a.clone(), session_b.clone()])
        );

        // Once session_a's floor elapses, both are admitted together: the
        // never-before-seen session_b was never inside a floor at all.
        tokio::time::advance(TARGETED_MIN_INTERVAL - Duration::from_secs(1)).await;
        let (run_now, deferred, _due) = floors.admit(pending, Instant::now());
        assert!(
            deferred.sessions.is_empty(),
            "b was never admitted before, so it is due immediately"
        );
        assert_eq!(run_now.sessions, BTreeSet::from([session_a, session_b]));
    }
}
