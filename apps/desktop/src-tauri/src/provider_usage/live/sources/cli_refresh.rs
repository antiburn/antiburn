// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ask the agent to refresh its own usage, then read what it wrote.
//!
//! # Why this exists
//!
//! The offline source reads figures the agent cached the last time *it* was
//! online. That is honest but it cannot be current on demand: a reader who
//! opens the Usage screen after an hour away sees an hour-old reading and no
//! way to improve it.
//!
//! This source is the way to improve it, and it is the only part of the
//! feature that causes any network traffic at all — indirectly, through a
//! child process. It is therefore opt-in and default off
//! ([`crate::store::AppSettings::live_usage_enabled`]), and the settings row
//! says in as many words what turning it on does.
//!
//! # What it does not do
//!
//! **It does not read the agent's output.** Running `/usage` prints a table of
//! percentages, and parsing it was the obvious design — the private app does
//! exactly that. Two things argued against it. The text states its resets in
//! IANA zone names, so believing them needs a timezone database, and
//! [`crate::provider_usage`] already records a deliberate decision against
//! taking that dependency. And parsing a child process's stdout into a number
//! this surface then presents as the provider's own word is a trust step worth
//! not taking when it buys nothing.
//!
//! It buys nothing because running the agent updates its cache as a side
//! effect — verified: the file's fetch stamp advances across an invocation.
//! So this source runs the agent, discards every byte it prints, and re-reads
//! the same file the offline source reads, through the same function. The
//! result is one reading shape, with the provider's own RFC 3339 resets, and
//! no parser to keep in step with a human-readable table.
//!
//! # Containment
//!
//! The child runs in a private working directory with an empty settings file,
//! and with the agent's own session-identifying variables removed from its
//! environment, so it cannot be mistaken for — or recurse into — the session
//! that launched antiburn. It is killed if it outstays its timeout.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use time::OffsetDateTime;

use super::super::model::ProviderUsageError;
use super::super::{LiveUsageSource, SourceOutcome};
use super::local_cache;

/// How long to wait for the agent before giving up and killing it.
///
/// Generous: it starts a runtime and makes a request. The cost of being wrong
/// in the tight direction is a refresh that never succeeds; in the loose
/// direction, a process lingering for a few more seconds.
const TIMEOUT: Duration = Duration::from_secs(45);

/// How long to leave the agent alone after a successful refresh.
///
/// Ten minutes, and the figure is set by what a refresh *costs* rather than
/// by how fast the numbers move. This is a whole agent process — a language
/// runtime, a request, a teardown — not an HTTP call, and the monitor asks for
/// one every five minutes while the opt-in is on. Against a five-hour window a
/// ten-minute-old reading is indistinguishable from a current one, and against
/// a weekly window it is not even close, so the shorter cooldown buys nothing
/// a reader could perceive and costs a process every tick.
const SUCCESS_COOLDOWN: Duration = Duration::from_secs(600);

/// …and after a failed one. Longer still, because whatever went wrong —
/// signed out, rate limited, agent not installed — is unlikely to have fixed
/// itself in the ten minutes a success would have bought, and retrying a
/// spawn that keeps failing is the worst version of this cost.
const FAILURE_COOLDOWN: Duration = Duration::from_secs(1_800);

/// Environment variables that tell the agent it is running inside itself.
///
/// Removed so the child is an ordinary invocation. Leaving them would let a
/// refresh launched from inside an agent session be attributed to that
/// session, or be refused outright.
const INHERITED_SESSION_VARS: [&str; 3] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
];

/// Runs the agent to refresh its cache, then reads the cache.
pub struct ClaudeCliRefresh {
    /// Where the agent writes what we read.
    cache: Option<PathBuf>,
    /// The private directory the child runs in.
    workspace: PathBuf,
    /// When the last attempt ran, and whether it worked.
    last: Mutex<Option<(Instant, bool)>>,
}

impl ClaudeCliRefresh {
    pub fn new(workspace: PathBuf) -> ClaudeCliRefresh {
        ClaudeCliRefresh {
            cache: local_cache::default_path(),
            workspace,
            last: Mutex::new(None),
        }
    }

    /// Whether enough time has passed to try again.
    fn off_cooldown(&self) -> bool {
        let last = self
            .last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *last {
            None => true,
            Some((at, succeeded)) => {
                at.elapsed()
                    >= if succeeded {
                        SUCCESS_COOLDOWN
                    } else {
                        FAILURE_COOLDOWN
                    }
            }
        }
    }

    fn remember(&self, succeeded: bool) {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *last = Some((Instant::now(), succeeded));
    }

    /// Run the agent once. Returns whether it exited cleanly.
    ///
    /// Every byte it prints is discarded — see the module doc. The only thing
    /// this call is for is the side effect on the agent's own cache file.
    fn run(&self) -> bool {
        if std::fs::create_dir_all(&self.workspace).is_err() {
            return false;
        }
        // An empty settings file, so the child inherits none of the reader's
        // project configuration and writes none of its own.
        if std::fs::write(self.workspace.join("settings.json"), "{}").is_err() {
            return false;
        }

        let mut command = Command::new("claude");
        command
            .arg("--settings")
            .arg(self.workspace.join("settings.json"))
            .arg("--no-session-persistence")
            .arg("--print")
            .arg("/usage")
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for name in INHERITED_SESSION_VARS {
            command.env_remove(name);
        }

        let Ok(mut child) = command.spawn() else {
            return false;
        };

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if started.elapsed() >= TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(250)),
                Err(_) => return false,
            }
        }
    }
}

impl LiveUsageSource for ClaudeCliRefresh {
    fn id(&self) -> &'static str {
        "claude-cli-refresh"
    }

    /// True only for the online opt-in. Nothing here runs while it is off.
    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn fetch(&self) -> SourceOutcome {
        if !self.off_cooldown() {
            // Still inside a cooldown. Report nothing rather than an error:
            // the offline source is reading the same file on this very pass,
            // so the reader loses nothing and gains no spurious warning.
            return SourceOutcome::absent();
        }

        let refreshed = self.run();
        self.remember(refreshed);
        if !refreshed {
            // The agent is missing, signed out, or refused. We cannot tell
            // which from an exit code, and guessing between them would put a
            // confident wrong instruction on the reader's screen.
            return SourceOutcome::failed(ProviderUsageError::Unavailable);
        }

        local_cache::read(self.cache.as_deref(), OffsetDateTime::now_utc())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ClaudeCliRefresh {
        ClaudeCliRefresh::new(std::env::temp_dir().join("antiburn-refresh-test"))
    }

    #[test]
    fn a_fresh_source_is_ready_and_a_just_run_one_is_not() {
        let source = source();
        assert!(source.off_cooldown());

        source.remember(true);
        assert!(!source.off_cooldown());

        source.remember(false);
        assert!(!source.off_cooldown());
    }

    #[test]
    fn a_failure_waits_longer_than_a_success() {
        // Whatever went wrong is unlikely to have fixed itself in the two
        // minutes a success would have bought.
        assert!(FAILURE_COOLDOWN > SUCCESS_COOLDOWN);
    }

    #[test]
    fn the_source_declares_itself_online_so_the_gate_can_find_it() {
        // The gate is declarative: `collect` asks each source whether it needs
        // the opt-in rather than knowing their names. A source that forgot to
        // say so would run with the preference off, which is the one failure
        // this whole design exists to prevent.
        assert!(source().requires_online_opt_in());
        assert!(!super::super::local_cache::ClaudeLocalCache::new().requires_online_opt_in());
    }
}
