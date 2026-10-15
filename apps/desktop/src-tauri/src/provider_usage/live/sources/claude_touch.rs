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
//! holds a minutes-scale cooldown between attempts — [`TOUCH_COOLDOWN`] for
//! a user-initiated check and the longer [`BACKGROUND_TOUCH_COOLDOWN`] for a
//! background check — plus in-flight dedup, and a *terminal* state: when a
//! settled refresh still produced a dead credential, the CLI's refresh token
//! itself is bad (`invalid_grant`), waiting will not fix it, and the touch
//! stays blocked until the credential material on disk changes — that is,
//! until the reader runs `claude` and signs in again.
//!
//! An attempt that does not settle also counts against the material. A
//! login that cannot refresh (a blank login, or a login without a refresh
//! token) and a live login that the endpoint rejected get one attempt per
//! change of the material. An expired login with a refresh token gets
//! [`UNCHANGED_ATTEMPT_LIMIT`] attempts. After the limit, the gate marks the
//! material terminal. A refresh before expiry never counts. See
//! [`TouchRequest::attempt_limit`].
//!
//! # Retrying an attribute read
//!
//! An attribute read can fail for a short time, for example while the CLI
//! writes the item. [`with_retries`] retries such a read with a short
//! backoff ([`RETRY_DELAYS`]). The verification polls and the source's
//! change-marker check use the same helper.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::cli_locator;

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
pub(super) const TOUCH_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// The wait between attempts that background checks start.
///
/// The background monitor checks every five minutes. A longer wait keeps a
/// login that does not recover from a PTY spawn on every tick. A
/// user-initiated check still uses [`TOUCH_COOLDOWN`].
pub(super) const BACKGROUND_TOUCH_COOLDOWN: Duration = Duration::from_secs(10 * 60);

/// How many attempts at the same material can end unchanged before an
/// expired login with a refresh token needs a new sign-in. The CLI refreshes
/// an expired token on its first run, so more unchanged runs show that the
/// refresh token is bad.
pub(super) const UNCHANGED_ATTEMPT_LIMIT: u32 = 3;

/// The waits before each retry of a failed attribute read. Three retries
/// follow the first attempt, so a read is tried four times in total.
pub(super) const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// Run `attempt` until it returns a value, at most once plus once per
/// [`RETRY_DELAYS`] entry. `sleep` waits before each retry; a test supplies
/// a sleep that returns at once.
///
/// `Err` holds the number of attempts that failed.
pub(super) fn with_retries<T>(
    sleep: &dyn Fn(Duration),
    mut attempt: impl FnMut() -> Option<T>,
) -> Result<T, u32> {
    let mut attempts = 1_u32;
    if let Some(value) = attempt() {
        return Ok(value);
    }
    for delay in RETRY_DELAYS {
        sleep(delay);
        attempts += 1;
        if let Some(value) = attempt() {
            return Ok(value);
        }
    }
    Err(attempts)
}

/// How one touch attempt runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchRequest {
    /// The wait since the last attempt before a new attempt may start.
    pub cooldown: Duration,
    /// How many attempts at the same material can end unchanged. The
    /// attempt that reaches the limit marks the material terminal, so the
    /// next attempt waits for the material to change. `None` never marks it,
    /// for a refresh before expiry of a login that still works.
    pub attempt_limit: Option<u32>,
}

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
    /// No `claude` binary exists to spawn. Nothing here can refresh the
    /// credential.
    CliMissing,
    /// The carrier still matches a refresh that produced a dead credential.
    /// Only a new sign-in changes this.
    Terminal,
    /// The touch did not start: the cooldown or another attempt blocked it,
    /// the material could not be observed, or the spawn failed. A later
    /// check can try again.
    Skipped,
    /// The CLI ran, but the material did not change and settle inside the
    /// deadline. The credential is exactly as expired as it was.
    NotRefreshed,
}

/// Run one gated touch attempt end to end: probe, fingerprint, spawn,
/// verify, tear down.
///
/// The child is killed as soon as verification concludes, settled or not —
/// the hard deadline in [`MAX_POLLS`] is the child's lifetime cap.
pub fn touch(env: &dyn TouchEnvironment, gate: &TouchGate, request: TouchRequest) -> TouchOutcome {
    if !env.binary_present() {
        log_touch_outcome("cli_missing");
        return TouchOutcome::CliMissing;
    }
    let Some(before) = env.fingerprint() else {
        log_touch_outcome("metadata_unavailable");
        return TouchOutcome::Skipped;
    };
    if gate.is_terminal(&before) {
        log_touch_outcome("terminal");
        return TouchOutcome::Terminal;
    }
    if !gate.begin(&before, request.cooldown) {
        log_touch_outcome("cooldown_or_in_flight");
        return TouchOutcome::Skipped;
    }
    let Some(mut child) = env.spawn() else {
        gate.finish();
        log_touch_outcome("spawn_failed");
        return TouchOutcome::Skipped;
    };
    let settled = verify(env, &before);
    child.kill();
    gate.finish();
    match settled {
        Some(fingerprint) => {
            gate.clear_unchanged();
            log_touch_outcome("settled");
            TouchOutcome::Settled(fingerprint)
        }
        None => {
            log_touch_outcome("verification_timeout");
            if let Some(limit) = request.attempt_limit
                && gate.note_unchanged(&before) >= limit
            {
                // More attempts do not repair this login. Block them until
                // the reader signs in again.
                gate.mark_terminal(before);
                return TouchOutcome::Terminal;
            }
            TouchOutcome::NotRefreshed
        }
    }
}

fn log_touch_outcome(outcome: &'static str) {
    ::tracing::debug!(event = "claude_refresh_outcome", outcome);
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
    /// The material of the last attempts that ended unchanged, and how many
    /// attempts in a row did.
    unchanged: Option<(Fingerprint, u32)>,
}

impl TouchGate {
    pub fn new() -> TouchGate {
        TouchGate::default()
    }

    /// Whether a touch may start against the carrier's current fingerprint.
    /// `true` claims the in-flight slot and stamps the attempt.
    fn begin(&self, before: &Fingerprint, cooldown: Duration) -> bool {
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
        if inner.last_attempt.is_some_and(|at| at.elapsed() < cooldown) {
            return false;
        }
        inner.in_flight = true;
        inner.last_attempt = Some(Instant::now());
        true
    }

    /// Release the in-flight slot. The attempt stamp stays: settled or not,
    /// the next touch waits out the cooldown.
    fn finish(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.in_flight = false;
    }

    /// Whether the carrier still matches a refresh that produced a dead
    /// credential — see [`GateInner::terminal`].
    fn is_terminal(&self, before: &Fingerprint) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .terminal
            .as_ref()
            == Some(before)
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

    /// Clear the terminal block and the unchanged count after the login
    /// worked again.
    pub fn clear_terminal(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.terminal = None;
        inner.unchanged = None;
    }

    /// Count one more attempt that ended unchanged at `before`. Returns the
    /// count in a row at this material.
    fn note_unchanged(&self, before: &Fingerprint) -> u32 {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = match &inner.unchanged {
            Some((material, count)) if material == before => count + 1,
            _ => 1,
        };
        inner.unchanged = Some((before.clone(), count));
        count
    }

    /// Forget the unchanged count after an attempt settled.
    fn clear_unchanged(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.unchanged = None;
    }

    /// Whether the gate blocks any material.
    #[cfg(test)]
    pub fn is_terminal_for_test(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .terminal
            .is_some()
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
pub(super) const CLAUDE_BINARY: &str = "claude";

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
        cli_locator::locate(CLAUDE_BINARY).is_some()
    }

    fn fingerprint(&self) -> Option<Fingerprint> {
        let mut parts = Vec::new();
        #[cfg(target_os = "macos")]
        match keychain_metadata_with_retries(&|delay| std::thread::sleep(delay)) {
            Ok(KeychainMetadata::Found(text)) => {
                parts.push(format!("keychain:{:016x}", hash(&text)));
            }
            Ok(KeychainMetadata::Absent) => parts.push("keychain:absent".to_owned()),
            Ok(KeychainMetadata::Unreadable) | Err(_) => return None,
        }
        let file = self.credentials_path.as_deref().map(|path| {
            with_retries(&|delay| std::thread::sleep(delay), || {
                match std::fs::read(path) {
                    Ok(bytes) => Some(Some(bytes)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
                    Err(_) => None,
                }
            })
        });
        match file {
            Some(Ok(Some(bytes))) => parts.push(format!("file:{:016x}", hash(&bytes))),
            Some(Err(_)) => parts.push("file:unreadable".to_owned()),
            Some(Ok(None)) | None => parts.push("file:absent".to_owned()),
        }
        Some(Fingerprint(parts.join(";")))
    }

    fn spawn(&self) -> Option<Box<dyn TouchChild>> {
        use std::io::{Read as _, Write as _};

        use portable_pty::{CommandBuilder, PtySize, native_pty_system};

        let (binary, dirs) = cli_locator::locate(CLAUDE_BINARY)?;
        let pty = native_pty_system().openpty(PtySize::default()).ok()?;
        let mut command = CommandBuilder::new(&binary);
        command.env("TERM", "xterm-256color");
        // An app started from Finder has a thin `PATH`. The CLI and any
        // `node` it needs must resolve without the reader's shell profile.
        if let Some(path) = cli_locator::child_path(&binary, &dirs) {
            command.env("PATH", path);
        }
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

/// A stable content hash for fingerprinting. Collision resistance is not a
/// requirement — the fingerprint only has to change when the bytes do.
pub(super) fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// What one metadata read of the Keychain item found. Mirrors
/// `anthropic_fetch`'s `KeychainRead`, for the `-w`-less read.
#[cfg(target_os = "macos")]
#[derive(Clone)]
pub(super) enum KeychainMetadata {
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
pub(super) fn keychain_metadata() -> KeychainMetadata {
    keychain_metadata_for("Claude Code-credentials", None)
}

/// Read the Claude item's attributes, with [`with_retries`] around a failed
/// read. `Ok` is never [`KeychainMetadata::Unreadable`]. `Err` holds the
/// number of attempts that failed.
#[cfg(target_os = "macos")]
pub(super) fn keychain_metadata_with_retries(
    sleep: &dyn Fn(Duration),
) -> Result<KeychainMetadata, u32> {
    retry_metadata(sleep, keychain_metadata)
}

/// Retry `read` while it reports [`KeychainMetadata::Unreadable`].
#[cfg(target_os = "macos")]
pub(super) fn retry_metadata(
    sleep: &dyn Fn(Duration),
    mut read: impl FnMut() -> KeychainMetadata,
) -> Result<KeychainMetadata, u32> {
    with_retries(sleep, || match read() {
        KeychainMetadata::Unreadable => None,
        metadata => Some(metadata),
    })
}

/// Read attributes for the selected service and account without requesting the secret.
#[cfg(target_os = "macos")]
pub(super) fn keychain_metadata_for(service: &str, account: Option<&str>) -> KeychainMetadata {
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

    let mut child = match keychain_metadata_command(service, account)
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

#[cfg(target_os = "macos")]
fn keychain_metadata_command(service: &str, account: Option<&str>) -> std::process::Command {
    let mut command = antiburn_local::platform::process::headless_std_command("security");
    command.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        command.args(["-a", account]);
    }
    command
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    const REQUEST: TouchRequest = TouchRequest {
        cooldown: TOUCH_COOLDOWN,
        attempt_limit: None,
    };

    #[cfg(target_os = "macos")]
    #[test]
    fn metadata_commands_select_attributes_without_requesting_secrets() {
        for (service, account, expected) in [
            (
                "Claude Code-credentials",
                None,
                vec!["find-generic-password", "-s", "Claude Code-credentials"],
            ),
            (
                "gemini",
                Some("antigravity"),
                vec!["find-generic-password", "-s", "gemini", "-a", "antigravity"],
            ),
        ] {
            let command = keychain_metadata_command(service, account);
            assert_eq!(command.get_program(), "security");
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        }
    }

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
        let outcome = touch(&env, &TouchGate::new(), REQUEST);
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
        assert_eq!(
            touch(&env, &TouchGate::new(), REQUEST),
            TouchOutcome::NotRefreshed
        );
        assert!(env.killed.load(Ordering::SeqCst));
    }

    #[test]
    fn a_carrier_that_never_changes_times_out_at_the_poll_cap() {
        let env = ScriptedEnv::new(&["a"]);
        assert_eq!(
            touch(&env, &TouchGate::new(), REQUEST),
            TouchOutcome::NotRefreshed
        );
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
        assert_eq!(
            touch(&env, &TouchGate::new(), REQUEST),
            TouchOutcome::CliMissing
        );
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
        assert_eq!(
            touch(&Blind, &TouchGate::new(), REQUEST),
            TouchOutcome::Skipped
        );
    }

    #[test]
    fn a_failed_spawn_releases_the_gate_but_keeps_the_cooldown() {
        let mut env = ScriptedEnv::new(&["a"]);
        env.spawn_fails = true;
        let gate = TouchGate::new();
        assert_eq!(touch(&env, &gate, REQUEST), TouchOutcome::Skipped);
        // The attempt stamp holds: a second immediate touch is on cooldown.
        assert!(!gate.begin(&Fingerprint("a".into()), TOUCH_COOLDOWN));
        gate.open_cooldown_for_test();
        // Not in-flight: with the cooldown opened, an attempt may start.
        assert!(gate.begin(&Fingerprint("a".into()), TOUCH_COOLDOWN));
    }

    #[test]
    fn the_gate_dedups_in_flight_attempts_and_cools_down_between_them() {
        let gate = TouchGate::new();
        let fingerprint = Fingerprint("a".into());
        assert!(gate.begin(&fingerprint, TOUCH_COOLDOWN));
        // In flight: no second attempt.
        assert!(!gate.begin(&fingerprint, TOUCH_COOLDOWN));
        gate.finish();
        // Finished, but inside `TOUCH_COOLDOWN`: still no second attempt.
        assert!(!gate.begin(&fingerprint, TOUCH_COOLDOWN));
        gate.open_cooldown_for_test();
        assert!(gate.begin(&fingerprint, TOUCH_COOLDOWN));
    }

    #[test]
    fn a_terminal_failure_blocks_until_the_credential_material_changes() {
        let gate = TouchGate::new();
        gate.mark_terminal(Fingerprint("dead".into()));
        gate.open_cooldown_for_test();
        // The carrier still matches the terminal fingerprint: waiting will
        // not fix `invalid_grant`, so no touch runs.
        assert!(!gate.begin(&Fingerprint("dead".into()), TOUCH_COOLDOWN));
        let env = ScriptedEnv::new(&["dead", "fresh"]);
        assert_eq!(touch(&env, &gate, REQUEST), TouchOutcome::Terminal);
        assert_eq!(env.spawns.load(Ordering::SeqCst), 0);
        // The reader signed in again — the material changed — so the block
        // lifts on its own.
        assert!(gate.begin(&Fingerprint("fresh".into()), TOUCH_COOLDOWN));
    }

    #[test]
    fn clearing_the_terminal_state_reopens_the_original_fingerprint() {
        let gate = TouchGate::new();
        gate.mark_terminal(Fingerprint("dead".into()));
        gate.clear_terminal();
        gate.open_cooldown_for_test();
        assert!(gate.begin(&Fingerprint("dead".into()), TOUCH_COOLDOWN));
    }

    #[test]
    fn a_background_attempt_waits_longer_than_a_user_attempt() {
        assert!(BACKGROUND_TOUCH_COOLDOWN > TOUCH_COOLDOWN);
        let gate = TouchGate::new();
        let fingerprint = Fingerprint("a".into());
        assert!(gate.begin(&fingerprint, BACKGROUND_TOUCH_COOLDOWN));
        gate.finish();
        assert!(!gate.begin(&fingerprint, BACKGROUND_TOUCH_COOLDOWN));
        // A zero cooldown shows that the stamp alone decides.
        assert!(gate.begin(&fingerprint, Duration::ZERO));
    }

    #[test]
    fn a_single_attempt_that_does_not_settle_blocks_the_same_material() {
        let request = TouchRequest {
            attempt_limit: Some(1),
            ..REQUEST
        };
        let gate = TouchGate::new();
        let env = ScriptedEnv::new(&["blank"]);
        assert_eq!(touch(&env, &gate, request), TouchOutcome::Terminal);
        gate.open_cooldown_for_test();
        let again = ScriptedEnv::new(&["blank"]);
        assert_eq!(touch(&again, &gate, request), TouchOutcome::Terminal);
        assert_eq!(again.spawns.load(Ordering::SeqCst), 0);
        // A refresh before expiry keeps trying after a timeout.
        let gate = TouchGate::new();
        assert_eq!(
            touch(&ScriptedEnv::new(&["a"]), &gate, REQUEST),
            TouchOutcome::NotRefreshed
        );
        gate.open_cooldown_for_test();
        assert!(gate.begin(&Fingerprint("a".into()), TOUCH_COOLDOWN));
    }

    #[test]
    fn an_expired_login_needs_a_sign_in_after_the_unchanged_limit() {
        let request = TouchRequest {
            attempt_limit: Some(UNCHANGED_ATTEMPT_LIMIT),
            ..REQUEST
        };
        let gate = TouchGate::new();
        for _ in 1..UNCHANGED_ATTEMPT_LIMIT {
            assert_eq!(
                touch(&ScriptedEnv::new(&["dead"]), &gate, request),
                TouchOutcome::NotRefreshed
            );
            gate.open_cooldown_for_test();
        }
        assert_eq!(
            touch(&ScriptedEnv::new(&["dead"]), &gate, request),
            TouchOutcome::Terminal
        );
        gate.open_cooldown_for_test();
        let blocked = ScriptedEnv::new(&["dead"]);
        assert_eq!(touch(&blocked, &gate, request), TouchOutcome::Terminal);
        assert_eq!(blocked.spawns.load(Ordering::SeqCst), 0);
        // New material starts a new count.
        assert_eq!(
            touch(&ScriptedEnv::new(&["new"]), &gate, request),
            TouchOutcome::NotRefreshed
        );
    }

    #[test]
    fn a_working_login_resets_the_unchanged_count() {
        let request = TouchRequest {
            attempt_limit: Some(2),
            ..REQUEST
        };
        let gate = TouchGate::new();
        assert_eq!(
            touch(&ScriptedEnv::new(&["a"]), &gate, request),
            TouchOutcome::NotRefreshed
        );
        gate.clear_terminal();
        gate.open_cooldown_for_test();
        assert_eq!(
            touch(&ScriptedEnv::new(&["a"]), &gate, request),
            TouchOutcome::NotRefreshed
        );
    }

    #[test]
    fn a_failed_read_is_retried_three_times_with_backoff() {
        let waits = Mutex::new(Vec::new());
        let sleep = |delay: Duration| waits.lock().unwrap().push(delay);
        let calls = AtomicUsize::new(0);
        let result: Result<(), u32> = with_retries(&sleep, || {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        });
        assert_eq!(result, Err(4));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(*waits.lock().unwrap(), RETRY_DELAYS.to_vec());

        let waits = Mutex::new(Vec::new());
        let sleep = |delay: Duration| waits.lock().unwrap().push(delay);
        let calls = AtomicUsize::new(0);
        let result = with_retries(&sleep, || {
            (calls.fetch_add(1, Ordering::SeqCst) == 1).then_some("found")
        });
        assert_eq!(result, Ok("found"));
        assert_eq!(*waits.lock().unwrap(), vec![RETRY_DELAYS[0]]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_absent_item_is_an_answer_and_is_not_retried() {
        let calls = AtomicUsize::new(0);
        let result = retry_metadata(&|_| {}, || {
            calls.fetch_add(1, Ordering::SeqCst);
            KeychainMetadata::Absent
        });
        assert!(matches!(result, Ok(KeychainMetadata::Absent)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let result = retry_metadata(&|_| {}, || KeychainMetadata::Unreadable);
        assert!(matches!(result, Err(4)));
    }
}
