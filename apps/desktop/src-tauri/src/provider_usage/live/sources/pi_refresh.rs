//! Delegate an expired Pi credential's refresh to Pi's own CLI.
//!
//! [`super::pi_auth`] reads Pi's OAuth store as a read-only carrier, and
//! that module never refreshes a token. It must not: Anthropic and OpenAI
//! rotate the refresh token on redemption, and Pi owns that lifecycle — a
//! second redeemer racing Pi's own would strand one of them with a dead
//! token. On a machine where only Pi is installed, an expired entry
//! therefore means both usage rows go dark until the reader next uses Pi.
//!
//! This module is the lever for that machine. It does not redeem the
//! refresh token either. Instead it runs Pi's own upstream command,
//! `pi auth check --provider <p> --json`, which — by default, without
//! `--no-refresh` — refreshes an expired OAuth token through the SDK's
//! `ModelRuntime.getAuth(providerId)`, inside `CredentialStore.modify` — a
//! serialized read-modify-write under a **cross-process file lock**, so it
//! cannot double-refresh against a live Pi session — and Pi itself
//! persists the rotated credential to `auth.json`. The owner's code
//! performs the redeem and the write; this module only asks. `auth check`
//! makes no model request and spends no tokens.
//!
//! # The exchange
//!
//! 1. Locate the installed pi package (see [`locate_package_cli_in`]) and
//!    a `node` binary — node is present on any machine that has Pi, because
//!    Pi is a Node.js package.
//! 2. Spawn `node <pi-cli> auth check --provider <provider> --json` under
//!    [`TIMEOUT`]. Without `--no-refresh`, `auth check` refreshes an
//!    expired OAuth credential through `ModelRuntime.getAuth` — the same
//!    locked SDK path described above — and reports `{"status":...}` on
//!    stdout. The CLI is Pi's own stable surface for exactly this ask, so
//!    no SDK import of ours can drift out from under it.
//! 3. Verify: the store's bytes are fingerprinted before the spawn, and read
//!    once more after a clean exit. The process exit *is* the completion
//!    signal — the CLI exits only after `getAuth`'s locked write has landed
//!    — so no quiescence polling is needed. A changed fingerprint means a
//!    rotated credential: the caller re-reads the entry and retries its
//!    usage call once. An unchanged fingerprint on a clean exit means the
//!    token was already valid, and the current entry proceeds as-is.
//!
//! # Failure is always "lever unavailable"
//!
//! pi not found, a package-name mismatch, no node, a CLI too old to know
//! `auth check`, an `"invalid"` runtime state, a timeout, a crash, an
//! unrecognized answer — every one of these reads as
//! [`Recovery::Unavailable`], and the caller falls through to exactly
//! today's behavior. This lever is strictly additive; its worst case must
//! equal its absence. The one exception is an answer of
//! `{"status":"not_ready"}`: Pi's own CLI ran to completion, tried the
//! refresh, and could not produce a credential, which is a terminal
//! rejection, not a transient failure. That maps to
//! [`Recovery::SignInWithPi`] — the fix is signing in again with Pi, and
//! the caller reports it as an authentication failure.
//!
//! # Not a request storm
//!
//! One expired credential must not spawn node repeatedly. Attempts are
//! gated by [`REFRESH_COOLDOWN`] — the same ceiling
//! [`super::cooldown::FAILURE_COOLDOWN`] puts on a failed fetch's retry —
//! and the gate is claimed *before* the spawn, so a caller arriving while an
//! attempt is in flight deduplicates onto it instead of starting another.
//!
//! # Testability
//!
//! [`RefreshRunner`] is the seam: [`PiRefresher::recover`] is written
//! against the trait, so tests exercise the fingerprint-and-verify contract
//! with a fake that never spawns node — the same pattern the neighboring
//! sources use for their transports.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::cooldown;
use super::pi_auth;

/// The npm package name the located install must carry. Anything else on
/// the reader's PATH that happens to be called `pi` is not asked to run.
const PI_PACKAGE_NAME: &str = "@earendil-works/pi-coding-agent";

/// The whole delegated exchange — node start, one `auth check`, one locked
/// refresh — must land inside this. Past it, the child is killed and the
/// lever reads as unavailable.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The minimum wait between attempts, successful or not. Reuses the shared
/// failure-cooldown ceiling: a node spawn costs more than the HTTP retries
/// that constant already governs, so it never waits less than they do.
const REFRESH_COOLDOWN: Duration = cooldown::FAILURE_COOLDOWN;

/// A cap on the child's stdout. `auth check --json` legitimately prints
/// one short JSON line; anything near this cap is not that.
const MAX_STDOUT_BYTES: usize = 64 * 1024;

/// Matches [`pi_auth`]'s own read cap — the store this fingerprints is the
/// same small credential file that module reads.
const MAX_STORE_BYTES: u64 = 256 * 1024;

/// A cap on a candidate `package.json` read during location.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// What one delegated run of Pi's SDK reported, before the store is
/// re-examined. The spawn boundary's whole vocabulary — see
/// [`RefreshRunner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Exit 0, `{"status":"ready"}`: the CLI resolved a credential,
    /// refreshing it first when it had expired.
    Completed,
    /// `{"status":"not_ready"}`: Pi's own CLI ran, tried the refresh, and
    /// answered that no credential could be produced. Terminal, not
    /// transient.
    Rejected,
    /// Everything else. The lever could not run; behave as if it does not
    /// exist.
    Unavailable,
}

/// The spawn boundary, as a trait so tests supply it without node or pi
/// installed — the same seam shape as the neighboring sources' transports.
pub trait RefreshRunner: Send + Sync {
    /// Run the delegated refresh for `provider_key` and report how it ended.
    fn run(&self, provider_key: &str) -> RunOutcome;
}

/// What [`PiRefresher::recover`] tells its caller to do next.
///
/// No `Debug`: [`Recovery::Fresh`] carries live tokens, and a debug print
/// must not be a way to leak them into a log.
pub enum Recovery {
    /// The store changed and re-reads to a live entry: retry the usage call
    /// once with it.
    Fresh(pi_auth::PiOauth),
    /// Clean run, unchanged store: the token was already valid, so the
    /// entry the caller holds proceeds as-is.
    AlreadyValid,
    /// Pi's own SDK terminally rejected the refresh. Report an
    /// authentication failure — the fix is signing in again with Pi.
    SignInWithPi,
    /// The lever could not help this time. Fall through to today's
    /// behavior.
    Unavailable,
}

/// One provider entry's delegated-refresh state: the runner, and the
/// cooldown gate that keeps one expired credential from spawning node
/// repeatedly.
pub struct PiRefresher {
    runner: Box<dyn RefreshRunner>,
    /// When the last attempt *started*. Claimed before the spawn, so an
    /// overlapping caller sees it and deduplicates instead of racing.
    last_attempt: Mutex<Option<Instant>>,
}

impl PiRefresher {
    /// The production refresher, backed by a real node spawn.
    pub fn new() -> PiRefresher {
        PiRefresher::with_runner(Box::new(LiveRunner))
    }

    /// A refresher over an explicit runner — the seam tests use.
    pub fn with_runner(runner: Box<dyn RefreshRunner>) -> PiRefresher {
        PiRefresher {
            runner,
            last_attempt: Mutex::new(None),
        }
    }

    /// A refresher whose lever is never available — what a test constructor
    /// installs so exercising a source cannot reach a real spawn.
    #[cfg(test)]
    pub fn unavailable() -> PiRefresher {
        struct Never;
        impl RefreshRunner for Never {
            fn run(&self, _provider_key: &str) -> RunOutcome {
                RunOutcome::Unavailable
            }
        }
        PiRefresher::with_runner(Box::new(Never))
    }

    /// Try to recover the expired entry at `auth_path` for `provider_key`.
    ///
    /// The caller has already decided the trigger holds: the entry is
    /// expired with a non-empty refresh field, and the failure it is
    /// recovering from is expiry — never a network or 5xx failure. This
    /// method owns everything after that decision: the cooldown gate, the
    /// fingerprint, the spawn, and the verify described in the module doc.
    pub fn recover(&self, auth_path: &Path, provider_key: &str) -> Recovery {
        {
            let mut last = self
                .last_attempt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if last.is_some_and(|at| at.elapsed() < REFRESH_COOLDOWN) {
                return Recovery::Unavailable;
            }
            // Claimed before the spawn: an overlapping caller lands here,
            // sees a fresh claim, and deduplicates.
            *last = Some(Instant::now());
        }
        let before = read_store(auth_path);
        match self.runner.run(provider_key) {
            RunOutcome::Completed => {
                // One read, once, after the exit — the exit is the
                // completion signal, so nothing here polls for quiescence.
                if read_store(auth_path) == before {
                    Recovery::AlreadyValid
                } else {
                    match pi_auth::read_entry(auth_path, provider_key) {
                        Some(entry) => Recovery::Fresh(entry),
                        // The store changed into a shape without this
                        // entry — a concurrent sign-out, say. Nothing to
                        // retry with.
                        None => Recovery::Unavailable,
                    }
                }
            }
            RunOutcome::Rejected => {
                ::tracing::warn!(
                    event = "pi_refresh_rejected",
                    provider_key,
                    advice = "sign in again with pi"
                );
                Recovery::SignInWithPi
            }
            RunOutcome::Unavailable => Recovery::Unavailable,
        }
    }
}

impl Default for PiRefresher {
    fn default() -> PiRefresher {
        PiRefresher::new()
    }
}

/// The store's fingerprint: its exact bytes, under the same size cap
/// [`pi_auth`] reads with. The file is small, so comparing content directly
/// is both the simplest and the strictest content hash available.
fn read_store(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_STORE_BYTES {
        return None;
    }
    fs::read(path).ok()
}

/// The production runner: locate pi and node, then spawn the bundled
/// script.
struct LiveRunner;

impl RefreshRunner for LiveRunner {
    fn run(&self, provider_key: &str) -> RunOutcome {
        let Some(cli) = locate_package_cli_in(&candidate_dirs()) else {
            return RunOutcome::Unavailable;
        };
        let Some(node) = locate_node(&candidate_dirs()) else {
            return RunOutcome::Unavailable;
        };
        run_delegated(&node, &cli, provider_key)
    }
}

/// Every directory a pi or node binary is looked for in: the process PATH
/// first, then the shim locations a GUI app's thin PATH tends to miss —
/// volta, bun, npm-global, and the common Homebrew and system prefixes. The
/// volta package image's own `bin` is included directly because volta's
/// PATH shim is a dispatcher binary, not a symlink a realpath can follow.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = antiburn_local::paths::home_dir() {
        for shim in [".volta/bin", ".bun/bin", ".npm-global/bin", ".local/bin"] {
            dirs.push(home.join(shim));
        }
        dirs.push(
            home.join(".volta/tools/image/packages")
                .join(PI_PACKAGE_NAME)
                .join("bin"),
        );
    }
    for fixed in ["/usr/local/bin", "/opt/homebrew/bin"] {
        dirs.push(PathBuf::from(fixed));
    }
    dirs
}

/// The bin names a pi install answers to, across platforms.
const PI_BIN_NAMES: [&str; 3] = ["pi", "pi.cmd", "pi.exe"];

/// Find the installed pi package's own CLI script, searching `dirs`.
///
/// Two strategies per directory, in order:
///
/// 1. Realpath a `pi` bin found there and walk up its ancestors to a
///    directory whose `package.json` names [`PI_PACKAGE_NAME`] — the
///    symlink layout npm and bun both install.
/// 2. Check the npm prefix layouts relative to the directory itself —
///    `../lib/node_modules/<pkg>` (Unix prefixes) and
///    `node_modules/<pkg>` (Windows prefixes) — for shims a realpath
///    cannot follow.
///
/// Either way the name check is the guard: a stray binary called `pi` that
/// does not resolve to this exact package never runs. The verified
/// install's own CLI script is what gets spawned — never the shim itself —
/// so the thing that runs is the thing that was verified.
fn locate_package_cli_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        for name in PI_BIN_NAMES {
            if let Ok(real) = fs::canonicalize(dir.join(name))
                && let Some(cli) = real.ancestors().skip(1).find_map(verified_cli)
            {
                return Some(cli);
            }
        }
        for root in [
            dir.join("../lib/node_modules").join(PI_PACKAGE_NAME),
            dir.join("node_modules").join(PI_PACKAGE_NAME),
        ] {
            if let Some(cli) = verified_cli(&root) {
                return Some(cli);
            }
        }
    }
    None
}

/// The verified CLI script under `root`, or `None` when `root` is not a pi
/// package install: no manifest, the wrong package name, a `bin` shape
/// this resolver does not know, or a script file that does not exist.
fn verified_cli(root: &Path) -> Option<PathBuf> {
    let manifest_path = root.join("package.json");
    let metadata = fs::metadata(&manifest_path).ok()?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let contents = fs::read_to_string(&manifest_path).ok()?;
    let manifest: Value = serde_json::from_str(&contents).ok()?;
    if manifest.get("name").and_then(Value::as_str) != Some(PI_PACKAGE_NAME) {
        return None;
    }
    let cli = root.join(manifest_cli(&manifest)?);
    cli.is_file().then_some(cli)
}

/// Read the package's `pi` CLI script out of its manifest's `bin` field —
/// a bare string, or an object keyed by bin name. Any shape beyond these
/// reads as `None` rather than a guess: a layout this resolver does not
/// recognize is a lever that is not available.
fn manifest_cli(manifest: &Value) -> Option<&str> {
    let bin = manifest.get("bin")?;
    if let Some(script) = bin.as_str() {
        return Some(script);
    }
    bin.get("pi").and_then(Value::as_str)
}

/// Find a `node` binary in `dirs`. Node is present on any machine that has
/// pi — pi is a Node.js package — so failing here reads as pi effectively
/// absent too.
fn locate_node(dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        for name in ["node", "node.exe"] {
            let bin = dir.join(name);
            if bin.is_file() {
                return Some(bin);
            }
        }
    }
    None
}

/// Run `node <cli> auth check --provider <provider> --json` under
/// [`TIMEOUT`] and classify what came back. Without `--no-refresh`, this
/// command refreshes an expired OAuth credential through Pi's own locked
/// `getAuth` path and persists the rotation itself.
fn run_delegated(node: &Path, cli: &Path, provider_key: &str) -> RunOutcome {
    let mut command = antiburn_local::platform::process::headless_std_command(node);
    command
        .arg(cli)
        .args(["auth", "check", "--provider", provider_key, "--json"]);
    match bounded_run(&mut command, TIMEOUT) {
        BoundedRun::Exited { success, stdout } => classify_output(success, &stdout),
        BoundedRun::Failed => RunOutcome::Unavailable,
    }
}

/// How one bounded child run ended.
enum BoundedRun {
    /// The child exited on its own, inside the deadline.
    Exited {
        /// Whether the exit status was zero.
        success: bool,
        /// Everything the child printed to stdout, up to the cap.
        stdout: String,
    },
    /// The child could not be spawned, overran the deadline and was killed,
    /// or its output could not be read.
    Failed,
}

/// Spawn `command` and wait for it, killing it past `timeout`.
///
/// The same shape as `anthropic_fetch::macos_keychain`'s bounded read: a
/// dedicated thread drains stdout into a channel, the caller waits on the
/// channel with the deadline, and on timeout the child is killed rather
/// than awaited — the reader thread unblocks on its own once the pipe
/// closes.
fn bounded_run(command: &mut std::process::Command, timeout: Duration) -> BoundedRun {
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return BoundedRun::Failed,
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return BoundedRun::Failed;
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::with_capacity(4096);
        let mut chunk = [0_u8; 4096];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    buffer.extend_from_slice(&chunk[..read]);
                    if buffer.len() > MAX_STDOUT_BYTES {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(buffer);
    });
    let bytes = match rx.recv_timeout(timeout) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return BoundedRun::Failed;
        }
    };
    let Ok(status) = child.wait() else {
        return BoundedRun::Failed;
    };
    if bytes.len() > MAX_STDOUT_BYTES {
        return BoundedRun::Failed;
    }
    match String::from_utf8(bytes) {
        Ok(stdout) => BoundedRun::Exited {
            success: status.success(),
            stdout,
        },
        Err(_) => BoundedRun::Failed,
    }
}

/// Map an exited child to a [`RunOutcome`]. A pure function so the one
/// distinction the sign-in-again state depends on — an answered
/// `"not_ready"` versus every other failure — has tests that never spawn
/// node.
///
/// `auth check` exits 0 for `"ready"`, 1 for `"not_ready"`, and 2 for
/// `"invalid"`, so the answer is read off stdout — each line is tried,
/// because the CLI may log around it — and the exit status only has to
/// agree with `"ready"`. `"invalid"` is a broken runtime, not a refusal,
/// and an old CLI without `auth check` prints no status at all; both read
/// as unavailable.
fn classify_output(success: bool, stdout: &str) -> RunOutcome {
    for line in stdout.lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line.trim())
            && let Some(status) = value.get("status").and_then(Value::as_str)
        {
            return match status {
                "ready" if success => RunOutcome::Completed,
                "not_ready" => RunOutcome::Rejected,
                _ => RunOutcome::Unavailable,
            };
        }
    }
    RunOutcome::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A store body whose entry is expired — the state every recover test
    /// starts a fixture file from.
    const EXPIRED_STORE: &str = r#"{"anthropic":{"type":"oauth","access":"stale-access","refresh":"synthetic-refresh","expires":1}}"#;

    /// A store body carrying a rotated, far-future entry — what Pi's own
    /// locked write leaves behind after a real refresh.
    const ROTATED_STORE: &str = r#"{"anthropic":{"type":"oauth","access":"rotated-access","refresh":"rotated-refresh","expires":9223372036854775807}}"#;

    /// A runner that counts calls, optionally rewrites the store the way
    /// Pi's own write would, and answers a fixed outcome.
    struct FakeRunner {
        calls: Arc<AtomicUsize>,
        rewrite: Option<(PathBuf, &'static str)>,
        outcome: RunOutcome,
    }

    impl RefreshRunner for FakeRunner {
        fn run(&self, _provider_key: &str) -> RunOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some((path, body)) = &self.rewrite {
                fs::write(path, body).expect("rewrite store");
            }
            self.outcome
        }
    }

    fn fixture_store(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("auth.json");
        fs::write(&path, EXPIRED_STORE).expect("write store");
        path
    }

    #[test]
    fn a_changed_fingerprint_after_a_clean_run_yields_the_rotated_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_store(&dir);
        let refresher = PiRefresher::with_runner(Box::new(FakeRunner {
            calls: Arc::new(AtomicUsize::new(0)),
            rewrite: Some((path.clone(), ROTATED_STORE)),
            outcome: RunOutcome::Completed,
        }));

        match refresher.recover(&path, pi_auth::ANTHROPIC_KEY) {
            Recovery::Fresh(entry) => {
                assert_eq!(entry.access_token, "rotated-access");
            }
            _ => panic!("expected Fresh"),
        }
    }

    #[test]
    fn an_unchanged_fingerprint_after_a_clean_run_reads_as_already_valid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_store(&dir);
        let refresher = PiRefresher::with_runner(Box::new(FakeRunner {
            calls: Arc::new(AtomicUsize::new(0)),
            rewrite: None,
            outcome: RunOutcome::Completed,
        }));

        assert!(matches!(
            refresher.recover(&path, pi_auth::ANTHROPIC_KEY),
            Recovery::AlreadyValid
        ));
    }

    #[test]
    fn a_rejected_run_reads_as_sign_in_with_pi() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_store(&dir);
        let refresher = PiRefresher::with_runner(Box::new(FakeRunner {
            calls: Arc::new(AtomicUsize::new(0)),
            rewrite: None,
            outcome: RunOutcome::Rejected,
        }));

        assert!(matches!(
            refresher.recover(&path, pi_auth::ANTHROPIC_KEY),
            Recovery::SignInWithPi
        ));
    }

    #[test]
    fn an_unavailable_run_falls_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_store(&dir);
        let refresher = PiRefresher::unavailable();
        assert!(matches!(
            refresher.recover(&path, pi_auth::ANTHROPIC_KEY),
            Recovery::Unavailable
        ));
    }

    #[test]
    fn a_clean_run_that_removes_the_entry_has_nothing_to_retry_with() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_store(&dir);
        let refresher = PiRefresher::with_runner(Box::new(FakeRunner {
            calls: Arc::new(AtomicUsize::new(0)),
            rewrite: Some((path.clone(), "{}")),
            outcome: RunOutcome::Completed,
        }));

        assert!(matches!(
            refresher.recover(&path, pi_auth::ANTHROPIC_KEY),
            Recovery::Unavailable
        ));
    }

    #[test]
    fn a_second_recover_inside_the_cooldown_never_reaches_the_runner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fixture_store(&dir);
        let calls = Arc::new(AtomicUsize::new(0));
        let refresher = PiRefresher::with_runner(Box::new(FakeRunner {
            calls: Arc::clone(&calls),
            rewrite: None,
            outcome: RunOutcome::Unavailable,
        }));

        refresher.recover(&path, pi_auth::ANTHROPIC_KEY);
        let second = refresher.recover(&path, pi_auth::ANTHROPIC_KEY);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(second, Recovery::Unavailable));
    }

    #[test]
    fn classify_output_believes_only_an_answered_status() {
        const READY: &str =
            "{\"status\":\"ready\",\"provider\":\"anthropic\",\"authType\":\"oauth\"}\n";
        const NOT_READY: &str = "{\"status\":\"not_ready\",\"provider\":\"anthropic\",\"reason\":\"credentials_not_configured\"}\n";
        assert_eq!(classify_output(true, READY), RunOutcome::Completed);
        // `auth check` exits 1 for not_ready; the answer still counts.
        assert_eq!(classify_output(false, NOT_READY), RunOutcome::Rejected);
        // CLI log lines around the answer are skipped, not fatal.
        assert_eq!(
            classify_output(true, &format!("starting runtime\n{READY}")),
            RunOutcome::Completed
        );
        // A broken runtime is not a refusal.
        assert_eq!(
            classify_output(
                false,
                "{\"status\":\"invalid\",\"reason\":\"invalid_state\"}\n"
            ),
            RunOutcome::Unavailable
        );
        // An old CLI that does not know `auth check` answers no status.
        assert_eq!(
            classify_output(false, "no answer\n"),
            RunOutcome::Unavailable
        );
        // A `ready` answer must agree with a zero exit to be believed.
        assert_eq!(classify_output(false, READY), RunOutcome::Unavailable);
    }

    /// Builds a pi-shaped install under `root`: the package tree, its
    /// manifest, and a bin file the locator can realpath into it.
    fn write_package(root: &Path, name: &str) -> PathBuf {
        let package = root
            .join("lib/node_modules")
            .join("@earendil-works/pi-coding-agent");
        fs::create_dir_all(package.join("dist/bundle")).expect("mkdir");
        fs::write(
            package.join("package.json"),
            format!(
                r#"{{"name":"{name}","main":"./dist/index.js",
                  "bin":{{"pi":"dist/bundle/cli.js"}}}}"#
            ),
        )
        .expect("write manifest");
        fs::write(package.join("dist/bundle/cli.js"), "// cli").expect("write cli");
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).expect("mkdir bin");
        bin_dir
    }

    #[test]
    fn the_npm_prefix_layout_locates_the_verified_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = write_package(dir.path(), PI_PACKAGE_NAME);
        // No bin file at all: the `../lib/node_modules` prefix strategy
        // still finds the package from the bin directory.
        let cli = locate_package_cli_in(&[bin_dir]).expect("located");
        assert!(cli.ends_with("dist/bundle/cli.js"));
    }

    #[cfg(unix)]
    #[test]
    fn a_bin_symlink_realpaths_up_to_the_verified_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = write_package(dir.path(), PI_PACKAGE_NAME);
        std::os::unix::fs::symlink(
            dir.path()
                .join("lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"),
            bin_dir.join("pi"),
        )
        .expect("symlink");
        let cli = locate_package_cli_in(&[bin_dir]).expect("located");
        assert!(cli.ends_with("dist/bundle/cli.js"));
    }

    #[test]
    fn a_package_name_mismatch_is_never_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = write_package(dir.path(), "some-other-package");
        assert!(locate_package_cli_in(&[bin_dir]).is_none());
    }

    #[test]
    fn an_absent_pi_locates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(locate_package_cli_in(&[dir.path().to_path_buf()]).is_none());
    }

    #[test]
    fn manifest_cli_reads_every_documented_bin_shape() {
        let keyed: Value = serde_json::from_str(r#"{"bin":{"pi":"dist/bundle/cli.js"}}"#).unwrap();
        assert_eq!(manifest_cli(&keyed), Some("dist/bundle/cli.js"));

        let bare: Value = serde_json::from_str(r#"{"bin":"./cli.js"}"#).unwrap();
        assert_eq!(manifest_cli(&bare), Some("./cli.js"));

        let wrong_key: Value = serde_json::from_str(r#"{"bin":{"other":"./cli.js"}}"#).unwrap();
        assert_eq!(manifest_cli(&wrong_key), None);

        let nothing: Value = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(manifest_cli(&nothing), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_past_the_deadline_is_killed_not_awaited() {
        let started = Instant::now();
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("30");
        let run = bounded_run(&mut command, Duration::from_millis(200));
        assert!(matches!(run, BoundedRun::Failed));
        // Well under the 30 s the child asked for: the kill landed.
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_clean_child_reports_its_stdout_inside_the_deadline() {
        let mut command = std::process::Command::new("/bin/echo");
        command.arg("{\"status\":\"ready\"}");
        match bounded_run(&mut command, Duration::from_secs(5)) {
            BoundedRun::Exited { success, stdout } => {
                assert!(success);
                assert_eq!(classify_output(success, &stdout), RunOutcome::Completed);
            }
            BoundedRun::Failed => panic!("echo should exit cleanly"),
        }
    }
}
