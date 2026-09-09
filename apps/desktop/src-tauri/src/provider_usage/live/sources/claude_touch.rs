//! Delegate an expired Claude credential's refresh to the Claude CLI itself.
//!
//! # Why a touch, and why the CLI
//!
//! [`super::anthropic_fetch`] never refreshes the token it reads. Anthropic
//! rotates refresh tokens on redemption, and a background reader that
//! redeemed one would corrupt a credential store it does not own. The tool
//! that owns the token's lifecycle is the `claude` CLI — so when every
//! carrier has expired, this module makes *the CLI* refresh and persist its
//! own credential: it spawns `claude` in a PTY, types the lightweight
//! `/status` command — which triggers a token refresh without a model
//! request — and tears the process down again. No token of the CLI's is ever
//! redeemed, written, or held here.
//!
//! # Why a PTY, and why this crate
//!
//! The CLI refreshes its credential when it runs interactively; piped stdio
//! puts it in a non-interactive mode that does not. Nothing already in the
//! tree can allocate a PTY — `antiburn_local::platform::process` builds
//! pipe-backed children only — so this module uses `portable-pty`, the
//! smallest maintained crate that covers both PTYs this app ships to
//! (`openpty` on macOS, ConPTY on Windows) behind one interface.
//!
//! # Verification: poll metadata, never the secret
//!
//! The PTY gives no structured completion signal, so poll-until-stable is the
//! primary verification mechanism. The carrier is fingerprinted before the
//! spawn, then polled until *quiescent*: changed from the pre-spawn
//! fingerprint **and** re-observed unchanged for [`STABLE_POLLS`] consecutive
//! polls, because the CLI may write more than once and the first change must
//! not be trusted. The whole verification is capped at [`MAX_POLLS`] polls of
//! [`POLL_INTERVAL`] each — about fifteen seconds — and the child is killed
//! when it concludes, either way.
//!
//! The fingerprint is **metadata only**. On macOS it comes from
//! `security find-generic-password` *without* `-w`, which returns item
//! attributes (including the modification date) and never hits the item's
//! access-control list — so the polling loop cannot raise a Keychain prompt,
//! by construction, no matter how often it runs. The credentials-file
//! carrier is fingerprinted by content hash; files have no prompt to avoid.
//! The secret itself is read exactly once, by the caller, after the change
//! has settled.
//!
//! # The gate
//!
//! One expired credential must not spawn PTYs repeatedly. [`TouchGate`]
//! holds a minutes-scale cooldown between attempts — a fixed
//! [`TOUCH_COOLDOWN`], sitting above `cooldown.rs`'s failure ceiling the way
//! that module's floor/ceiling pattern bounds its own retries — plus
//! in-flight dedup, and a *terminal* state: when a settled refresh still
//! produced a dead credential, the CLI's refresh token itself is bad
//! (`invalid_grant`), waiting will not fix it, and the touch stays blocked
//! until the credential material on disk changes — that is, until the reader
//! runs `claude` and signs in again.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The floor under the verification poll interval. Each macOS poll spawns a
/// `security` subprocess; anything faster turns verification into a
/// subprocess storm.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The hard cap on verification polls — thirty polls of half a second, the
/// ~15 s deadline on the whole touch. Past it the refresh is treated as
/// transiently failed, never awaited further.
const MAX_POLLS: u32 = 30;

/// How many consecutive polls a changed fingerprint must be re-observed
/// unchanged before it counts as settled — about two seconds. The CLI may
/// write the carrier more than once; the first change is never accepted.
const STABLE_POLLS: u32 = 4;

/// The wait between attempts, whatever the caller's own `max_age` says.
///
/// Modeled on `cooldown.rs`'s floor/ceiling pattern, but fixed rather than
/// derived: a touch costs a PTY, a subprocess per verification poll, and up
/// to fifteen seconds of deadline, so even the popover's eager polling gets
/// exactly one attempt per five minutes.
const TOUCH_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// An opaque, metadata-only summary of the credential carrier's state.
///
/// Equality is the only operation: two equal fingerprints mean the carrier
/// has not changed. The value never contains the secret — see the module
/// doc's verification section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint(pub String);

/// A running touch child that can be torn down.
pub trait TouchChild {
    /// Kill the child and release its PTY. Idempotent in effect: a child
    /// that already exited is simply reaped.
    fn kill(&mut self);
}

/// Everything the touch needs from the outside world, as a trait so a test
/// supplies a scripted world — the same seam `anthropic_fetch`'s
/// `AnthropicTransport` gives that source.
pub trait TouchEnvironment: Send + Sync {
    /// Whether the `claude` binary exists to be touched at all.
    fn binary_present(&self) -> bool;

    /// The carrier's current metadata fingerprint, or `None` when the
    /// carrier cannot be observed — in which case no touch runs, because a
    /// refresh that cannot be verified must not be started.
    fn fingerprint(&self) -> Option<Fingerprint>;

    /// Start the PTY touch. `None` means the spawn itself failed.
    fn spawn(&self) -> Option<Box<dyn TouchChild>>;

    /// Wait between verification polls. A test's world makes this free.
    fn sleep(&self, interval: Duration);
}

/// What one touch attempt concluded.
#[derive(Debug, PartialEq, Eq)]
pub enum TouchOutcome {
    /// The carrier changed from its pre-spawn fingerprint and settled at the
    /// named one. The caller may now read the secret — once.
    Settled(Fingerprint),
    /// The touch was skipped, failed to spawn, or never verified inside the
    /// deadline. The credential is exactly as expired as it was.
    NotRefreshed,
}

/// Run one gated touch attempt end to end: probe, fingerprint, spawn,
/// verify, tear down.
///
/// The child is killed as soon as verification concludes, settled or not —
/// the hard deadline in [`MAX_POLLS`] is the child's lifetime cap.
pub fn touch(env: &dyn TouchEnvironment, gate: &TouchGate) -> TouchOutcome {
    if !env.binary_present() {
        return TouchOutcome::NotRefreshed;
    }
    let Some(before) = env.fingerprint() else {
        return TouchOutcome::NotRefreshed;
    };
    if !gate.begin(&before) {
        return TouchOutcome::NotRefreshed;
    }
    let Some(mut child) = env.spawn() else {
        gate.finish();
        return TouchOutcome::NotRefreshed;
    };
    let settled = verify(env, &before);
    child.kill();
    gate.finish();
    match settled {
        Some(fingerprint) => TouchOutcome::Settled(fingerprint),
        None => TouchOutcome::NotRefreshed,
    }
}

/// Poll the carrier until it is quiescent: changed from `before` and
/// re-observed unchanged for [`STABLE_POLLS`] consecutive polls. `None`
/// past [`MAX_POLLS`] polls — the deadline the module doc describes.
fn verify(env: &dyn TouchEnvironment, before: &Fingerprint) -> Option<Fingerprint> {
    let mut last: Option<Fingerprint> = None;
    let mut stable = 0_u32;
    for _ in 0..MAX_POLLS {
        env.sleep(POLL_INTERVAL);
        match env.fingerprint() {
            Some(current) if current != *before => {
                if last.as_ref() == Some(&current) {
                    stable += 1;
                    if stable >= STABLE_POLLS {
                        return Some(current);
                    }
                } else {
                    stable = 0;
                    last = Some(current);
                }
            }
            _ => {
                stable = 0;
                last = None;
            }
        }
    }
    None
}

/// The cooldown, dedup, and terminal state one source's touches share.
#[derive(Default)]
pub struct TouchGate {
    inner: Mutex<GateInner>,
}

#[derive(Default)]
struct GateInner {
    /// Whether a touch is running right now. Belt over the cooldown's
    /// braces: the caller's own `Cooldown::poll` already serializes fetches,
    /// but this gate must hold on its own.
    in_flight: bool,
    last_attempt: Option<Instant>,
    /// The settled fingerprint of a refresh that still produced a dead
    /// credential — an `invalid_grant`-style terminal failure. While the
    /// carrier still matches it, no touch runs: the fix is signing in with
    /// the CLI, not another touch.
    terminal: Option<Fingerprint>,
}

impl TouchGate {
    pub fn new() -> TouchGate {
        TouchGate::default()
    }

    /// Whether a touch may start against the carrier's current fingerprint.
    /// `true` claims the in-flight slot and stamps the attempt.
    fn begin(&self, before: &Fingerprint) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.in_flight {
            return false;
        }
        if inner.terminal.as_ref() == Some(before) {
            return false;
        }
        if inner
            .last_attempt
            .is_some_and(|at| at.elapsed() < TOUCH_COOLDOWN)
        {
            return false;
        }
        inner.in_flight = true;
        inner.last_attempt = Some(Instant::now());
        true
    }

    /// Release the in-flight slot. The attempt stamp stays: settled or not,
    /// the next touch waits out [`TOUCH_COOLDOWN`].
    fn finish(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.in_flight = false;
    }

    /// Record that the refresh settled at `fingerprint` and still produced a
    /// dead credential. Touches stay blocked while the carrier matches it —
    /// see [`GateInner::terminal`].
    pub fn mark_terminal(&self, fingerprint: Fingerprint) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.terminal = Some(fingerprint);
    }

    /// Clear the terminal block after a refresh produced a live credential.
    pub fn clear_terminal(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.terminal = None;
    }

    /// Backdate the cooldown so a test can attempt again immediately — the
    /// same trick `cooldown.rs`'s own tests use. Terminal state is kept.
    #[cfg(test)]
    pub fn open_cooldown_for_test(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.last_attempt = None;
    }
}

/// The production environment: the reader's real `PATH`, real carriers, and
/// a real PTY.
pub struct CliTouchEnvironment {
    /// The `.credentials.json` carrier this environment fingerprints,
    /// matching the path the owning source reads.
    credentials_path: Option<std::path::PathBuf>,
}

/// The binary the touch spawns — the CLI that owns the credential.
const CLAUDE_BINARY: &str = "claude";

/// How long the CLI gets to start before `/status` is typed at it. Typing
/// into a PTY nobody is reading yet is harmless; typing before the CLI's
/// input handling is up would be lost.
const STATUS_DELAY: Duration = Duration::from_secs(3);

impl CliTouchEnvironment {
    pub fn new(credentials_path: Option<std::path::PathBuf>) -> CliTouchEnvironment {
        CliTouchEnvironment { credentials_path }
    }
}

impl TouchEnvironment for CliTouchEnvironment {
    fn binary_present(&self) -> bool {
        binary_on_path(CLAUDE_BINARY)
    }

    fn fingerprint(&self) -> Option<Fingerprint> {
        let mut parts = Vec::new();
        #[cfg(target_os = "macos")]
        match keychain_metadata() {
            KeychainMetadata::Found(text) => parts.push(format!("keychain:{:016x}", hash(&text))),
            KeychainMetadata::Absent => parts.push("keychain:absent".to_owned()),
            KeychainMetadata::Unreadable => return None,
        }
        match self.credentials_path.as_deref().map(std::fs::read) {
            Some(Ok(bytes)) => parts.push(format!("file:{:016x}", hash(&bytes))),
            Some(Err(_)) | None => parts.push("file:absent".to_owned()),
        }
        Some(Fingerprint(parts.join(";")))
    }

    fn spawn(&self) -> Option<Box<dyn TouchChild>> {
        use std::io::{Read as _, Write as _};

        use portable_pty::{CommandBuilder, PtySize, native_pty_system};

        let pty = native_pty_system().openpty(PtySize::default()).ok()?;
        let mut command = CommandBuilder::new(CLAUDE_BINARY);
        command.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(command).ok()?;
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().ok()?;
        let mut writer = pty.master.take_writer().ok()?;
        // Drain the CLI's output so a full PTY buffer cannot stall it. The
        // thread exits when the PTY closes; the bytes are discarded unread.
        std::thread::spawn(move || {
            let mut sink = [0_u8; 4096];
            loop {
                match reader.read(&mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        // Type `/status` once the CLI has had time to start. Errors are
        // ignored: a killed child just closes the writer's other end.
        std::thread::spawn(move || {
            std::thread::sleep(STATUS_DELAY);
            let _ = writer.write_all(b"/status\r");
            let _ = writer.flush();
        });
        Some(Box::new(PtyTouchChild {
            child,
            master: Some(pty.master),
        }))
    }

    fn sleep(&self, interval: Duration) {
        std::thread::sleep(interval);
    }
}

/// The live PTY child. Killing it also drops the master side, which is what
/// unblocks the drain thread.
struct PtyTouchChild {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
}

impl TouchChild for PtyTouchChild {
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.master.take();
    }
}

/// Whether `binary` names an executable file on the reader's `PATH` — the
/// same resolution spawning it would use, without spawning anything.
fn binary_on_path(binary: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        if dir.as_os_str().is_empty() {
            return false;
        }
        #[cfg(target_os = "windows")]
        {
            ["exe", "cmd", "bat", "ps1"]
                .iter()
                .any(|extension| dir.join(format!("{binary}.{extension}")).is_file())
                || dir.join(binary).is_file()
        }
        #[cfg(not(target_os = "windows"))]
        dir.join(binary).is_file()
    })
}

/// A stable content hash for fingerprinting. Collision resistance is not a
/// requirement — the fingerprint only has to change when the bytes do.
fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// What one metadata read of the Keychain item found. Mirrors
/// `anthropic_fetch`'s `KeychainRead`, for the `-w`-less read.
#[cfg(target_os = "macos")]
enum KeychainMetadata {
    Found(Vec<u8>),
    Absent,
    Unreadable,
}

/// Read the Keychain item's attributes — never its secret.
///
/// `security find-generic-password` without `-w` prints the item's
/// attributes, including its modification date, and does not touch the
/// item's access-control list — so this read can never raise a prompt. The
/// same bounded-subprocess shape as `anthropic_fetch::macos_keychain::read`:
/// a reader thread, a hard deadline, and a kill on timeout.
#[cfg(target_os = "macos")]
fn keychain_metadata() -> KeychainMetadata {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::sync::mpsc;

    /// Matches `anthropic_fetch::macos_keychain::TIMEOUT` — a metadata read
    /// is instant or wedged, never merely slow.
    const TIMEOUT: Duration = Duration::from_secs(3);
    /// Attributes are a few hundred bytes; cap the read defensively.
    const MAX_BYTES: usize = 64 * 1024;
    /// `errSecItemNotFound`, the same exit the secret read classifies.
    const ITEM_NOT_FOUND_EXIT_CODE: i32 = 44;

    let mut child = match antiburn_local::platform::process::headless_std_command("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return KeychainMetadata::Unreadable,
    };
    let Some(mut stdout) = child.stdout.take() else {
        return KeychainMetadata::Unreadable;
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
                    if buffer.len() > MAX_BYTES {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(buffer);
    });
    let bytes = match rx.recv_timeout(TIMEOUT) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return KeychainMetadata::Unreadable;
        }
    };
    let Ok(status) = child.wait() else {
        return KeychainMetadata::Unreadable;
    };
    if !status.success() {
        return if status.code() == Some(ITEM_NOT_FOUND_EXIT_CODE) {
            KeychainMetadata::Absent
        } else {
            KeychainMetadata::Unreadable
        };
    }
    if bytes.len() > MAX_BYTES {
        return KeychainMetadata::Unreadable;
    }
    KeychainMetadata::Found(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    /// A scripted world: each `fingerprint` call pops the next value, and
    /// the last value repeats once the script runs out.
    struct ScriptedEnv {
        binary_present: bool,
        script: Mutex<Vec<Fingerprint>>,
        polls: AtomicUsize,
        spawns: AtomicUsize,
        spawn_fails: bool,
        killed: std::sync::Arc<AtomicBool>,
    }

    impl ScriptedEnv {
        fn new(script: &[&str]) -> ScriptedEnv {
            let mut script: Vec<Fingerprint> = script
                .iter()
                .map(|s| Fingerprint((*s).to_owned()))
                .collect();
            script.reverse();
            ScriptedEnv {
                binary_present: true,
                script: Mutex::new(script),
                polls: AtomicUsize::new(0),
                spawns: AtomicUsize::new(0),
                spawn_fails: false,
                killed: std::sync::Arc::new(AtomicBool::new(false)),
            }
        }
    }

    struct FlagChild(std::sync::Arc<AtomicBool>);

    impl TouchChild for FlagChild {
        fn kill(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl TouchEnvironment for ScriptedEnv {
        fn binary_present(&self) -> bool {
            self.binary_present
        }
        fn fingerprint(&self) -> Option<Fingerprint> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            let mut script = self.script.lock().unwrap();
            if script.len() > 1 {
                script.pop()
            } else {
                script.last().cloned()
            }
        }
        fn spawn(&self) -> Option<Box<dyn TouchChild>> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            if self.spawn_fails {
                None
            } else {
                Some(Box::new(FlagChild(std::sync::Arc::clone(&self.killed))))
            }
        }
        fn sleep(&self, _interval: Duration) {}
    }

    #[test]
    fn the_poll_floor_and_cap_bound_the_verification_deadline() {
        assert!(POLL_INTERVAL >= Duration::from_millis(500));
        let deadline = POLL_INTERVAL * MAX_POLLS;
        assert!(deadline >= Duration::from_secs(14));
        assert!(deadline <= Duration::from_secs(16));
    }

    #[test]
    fn a_settled_change_is_not_the_first_change() {
        // The carrier moves twice — `b`, then `c` — before settling. The
        // touch must report `c`, never the first change `b`.
        let env = ScriptedEnv::new(&["a", "b", "c"]);
        let outcome = touch(&env, &TouchGate::new());
        assert_eq!(outcome, TouchOutcome::Settled(Fingerprint("c".into())));
        assert_eq!(env.spawns.load(Ordering::SeqCst), 1);
        assert!(env.killed.load(Ordering::SeqCst));
        // One pre-spawn read, then at least the change plus its stability
        // confirmations.
        assert!(env.polls.load(Ordering::SeqCst) as u32 >= 2 + STABLE_POLLS);
    }

    #[test]
    fn a_change_that_reverts_never_settles() {
        // The carrier flaps back to its pre-spawn state and stays there:
        // whatever wrote it did not leave a new credential behind.
        let env = ScriptedEnv::new(&["a", "b", "a"]);
        assert_eq!(touch(&env, &TouchGate::new()), TouchOutcome::NotRefreshed);
        assert!(env.killed.load(Ordering::SeqCst));
    }

    #[test]
    fn a_carrier_that_never_changes_times_out_at_the_poll_cap() {
        let env = ScriptedEnv::new(&["a"]);
        assert_eq!(touch(&env, &TouchGate::new()), TouchOutcome::NotRefreshed);
        // One pre-spawn read plus exactly `MAX_POLLS` verification polls —
        // the hard cap that keeps macOS polling from becoming a subprocess
        // storm.
        assert_eq!(env.polls.load(Ordering::SeqCst) as u32, 1 + MAX_POLLS);
        assert!(env.killed.load(Ordering::SeqCst));
    }

    #[test]
    fn an_absent_binary_skips_the_touch_entirely() {
        let mut env = ScriptedEnv::new(&["a", "b"]);
        env.binary_present = false;
        assert_eq!(touch(&env, &TouchGate::new()), TouchOutcome::NotRefreshed);
        assert_eq!(env.spawns.load(Ordering::SeqCst), 0);
        assert_eq!(env.polls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_unobservable_carrier_skips_the_touch() {
        struct Blind;
        impl TouchEnvironment for Blind {
            fn binary_present(&self) -> bool {
                true
            }
            fn fingerprint(&self) -> Option<Fingerprint> {
                None
            }
            fn spawn(&self) -> Option<Box<dyn TouchChild>> {
                panic!("an unverifiable refresh must not be started");
            }
            fn sleep(&self, _interval: Duration) {}
        }
        assert_eq!(touch(&Blind, &TouchGate::new()), TouchOutcome::NotRefreshed);
    }

    #[test]
    fn a_failed_spawn_releases_the_gate_but_keeps_the_cooldown() {
        let mut env = ScriptedEnv::new(&["a"]);
        env.spawn_fails = true;
        let gate = TouchGate::new();
        assert_eq!(touch(&env, &gate), TouchOutcome::NotRefreshed);
        // The attempt stamp holds: a second immediate touch is on cooldown.
        assert!(!gate.begin(&Fingerprint("a".into())));
        gate.open_cooldown_for_test();
        // Not in-flight: with the cooldown opened, an attempt may start.
        assert!(gate.begin(&Fingerprint("a".into())));
    }

    #[test]
    fn the_gate_dedups_in_flight_attempts_and_cools_down_between_them() {
        let gate = TouchGate::new();
        let fingerprint = Fingerprint("a".into());
        assert!(gate.begin(&fingerprint));
        // In flight: no second attempt.
        assert!(!gate.begin(&fingerprint));
        gate.finish();
        // Finished, but inside `TOUCH_COOLDOWN`: still no second attempt.
        assert!(!gate.begin(&fingerprint));
        gate.open_cooldown_for_test();
        assert!(gate.begin(&fingerprint));
    }

    #[test]
    fn a_terminal_failure_blocks_until_the_credential_material_changes() {
        let gate = TouchGate::new();
        gate.mark_terminal(Fingerprint("dead".into()));
        gate.open_cooldown_for_test();
        // The carrier still matches the terminal fingerprint: waiting will
        // not fix `invalid_grant`, so no touch runs.
        assert!(!gate.begin(&Fingerprint("dead".into())));
        // The reader signed in again — the material changed — so the block
        // lifts on its own.
        assert!(gate.begin(&Fingerprint("fresh".into())));
    }

    #[test]
    fn clearing_the_terminal_state_reopens_the_original_fingerprint() {
        let gate = TouchGate::new();
        gate.mark_terminal(Fingerprint("dead".into()));
        gate.clear_terminal();
        gate.open_cooldown_for_test();
        assert!(gate.begin(&Fingerprint("dead".into())));
    }

    #[test]
    fn an_absent_path_reads_as_no_binary() {
        // A name that cannot exist on any real `PATH`.
        assert!(!binary_on_path("antiburn-nonexistent-touch-binary"));
    }
}
