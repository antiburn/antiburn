// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded, window-free child processes.
//!
//! The engine reads local state by running a small, fixed set of local tools
//! (`git`, and `wsl.exe` on Windows). Those children must never flash a console
//! window in a GUI application, must never outlive their deadline, and must
//! never be able to exhaust memory through unbounded output.
//!
//! On Windows the command constructor can route children through a *process
//! host*: a re-invocation of a console-subsystem binary chosen by the embedding
//! application through [`set_process_host_resolver`]. The host owns the target
//! in a kill-on-close Job Object, so tearing down the direct child also tears
//! down its descendants. When no resolver is installed the target is spawned
//! directly.

use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::ffi::OsString;
use std::io;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[cfg(target_os = "windows")]
#[path = "process_windows.rs"]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::allocate_windowless_console;

/// Marks a re-invocation of the host binary. Accepted only as the first
/// argument so an ordinary argument can never be mistaken for it.
#[cfg(target_os = "windows")]
const INTERNAL_PROCESS_HOST_MARKER: &str = "--antiburn-local-internal-process-host-v1";

/// Resolves the console-subsystem executable used as the process host, or
/// `None` to spawn targets directly.
#[cfg(target_os = "windows")]
static PROCESS_HOST_RESOLVER: std::sync::OnceLock<fn() -> Option<OsString>> =
    std::sync::OnceLock::new();

/// Install the process-host resolver. Only the first call takes effect.
///
/// The resolver runs on every spawn, so it can decide per-process (for example,
/// only when the current executable is the GUI binary). Without it, children are
/// spawned directly.
#[cfg(target_os = "windows")]
pub fn set_process_host_resolver(resolver: fn() -> Option<OsString>) {
    let _ = PROCESS_HOST_RESOLVER.set(resolver);
}

#[cfg(target_os = "windows")]
fn process_host_executable() -> Option<OsString> {
    (PROCESS_HOST_RESOLVER.get()?)()
}

/// Build a Tokio child process that shows no console window in a GUI process.
pub fn headless_tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let program = program.as_ref();
    #[cfg(target_os = "windows")]
    let mut command = if let Some(host_executable) = process_host_executable() {
        let mut command = tokio::process::Command::new(host_executable);
        let (host_args, masked_path) = windows::prepare_host(program);
        command.arg(INTERNAL_PROCESS_HOST_MARKER);
        command.args(host_args);
        if let Some(masked_path) = masked_path {
            command.env("PATH", masked_path);
        }
        command
    } else {
        tokio::process::Command::new(program)
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = tokio::process::Command::new(program);
    hide_tokio_window(&mut command);
    command
}

/// Run the Windows process host before the application initializes.
///
/// Returns `None` for normal startup and the target's exit code for a host
/// invocation. The marker is intentionally accepted only as the first argument.
#[cfg(target_os = "windows")]
pub fn internal_process_host_exit_code() -> Option<i32> {
    let target_args = internal_process_host_args(std::env::args_os())?;
    Some(match windows::run(&target_args) {
        Ok(code) => code as i32,
        Err(error) => {
            let target = windows::target_program(&target_args)
                .and_then(|value| std::path::Path::new(value).file_name())
                .unwrap_or_else(|| OsStr::new("target process"));
            eprintln!(
                "process host could not launch {}: {error}",
                target.to_string_lossy()
            );
            1
        }
    })
}

#[cfg(target_os = "windows")]
fn internal_process_host_args(args: impl IntoIterator<Item = OsString>) -> Option<Vec<OsString>> {
    let mut args = args.into_iter();
    let _executable = args.next();
    (args.next().as_deref() == Some(OsStr::new(INTERNAL_PROCESS_HOST_MARKER)))
        .then(|| args.collect())
}

/// Build a std child process that shows no console window in a GUI process.
pub fn headless_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    hide_std_window(&mut command);
    command
}

/// Result of a bounded child process run.
#[derive(Debug)]
pub struct BoundedOutput {
    /// Child exit status.
    pub status: std::process::ExitStatus,
    /// Captured stdout, limited by [`BoundedOutputOptions::max_stdout_bytes`].
    pub stdout: Vec<u8>,
    /// Whether either captured stream exceeded its configured limit.
    pub truncated: bool,
    /// Whether stdout exceeded its configured limit.
    pub stdout_truncated: bool,
    /// Whether stderr exceeded its configured limit.
    pub stderr_truncated: bool,
}

/// Limits and environment changes for a bounded headless child process.
pub struct BoundedOutputOptions<'a> {
    /// Hard deadline for the child and its output streams.
    pub timeout: Duration,
    /// Maximum stdout bytes retained while the remainder is drained.
    pub max_stdout_bytes: usize,
    /// Stderr byte threshold beyond which the output is marked truncated.
    pub max_stderr_bytes: usize,
    /// Environment variables added or replaced for the child.
    pub env: &'a [(&'a str, &'a str)],
    /// Environment variables removed from the child.
    pub env_remove: &'a [&'a str],
    /// Working directory for the child, or the parent's directory when absent.
    pub current_dir: Option<&'a Path>,
}

/// Run a headless child with a hard deadline and bounded captured output.
///
/// When a Windows process host is installed, its kill-on-close job owns the
/// complete target process tree, so killing or dropping this direct child also
/// tears down descendants.
pub async fn bounded_headless_output(
    program: impl AsRef<OsStr>,
    args: &[&str],
    timeout: Duration,
    max_output_bytes: usize,
) -> io::Result<BoundedOutput> {
    bounded_headless_output_with_stdin(
        program,
        args,
        None,
        BoundedOutputOptions {
            timeout,
            max_stdout_bytes: max_output_bytes,
            max_stderr_bytes: max_output_bytes,
            env: &[],
            env_remove: &[],
            current_dir: None,
        },
    )
    .await
}

/// Run a headless child with optional stdin and independently bounded output streams.
pub async fn bounded_headless_output_with_stdin(
    program: impl AsRef<OsStr>,
    args: &[&str],
    stdin: Option<&[u8]>,
    options: BoundedOutputOptions<'_>,
) -> io::Result<BoundedOutput> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn drain_bounded<R: tokio::io::AsyncRead + Unpin>(
        mut reader: R,
        limit: usize,
    ) -> io::Result<(Vec<u8>, bool)> {
        let mut kept = Vec::with_capacity(limit.min(8192));
        let mut buf = [0_u8; 8192];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            let remaining = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&buf[..read.min(remaining)]);
            truncated |= read > remaining;
        }
        Ok((kept, truncated))
    }

    let mut command = headless_tokio_command(program);
    #[cfg(unix)]
    command.process_group(0);
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.envs(options.env.iter().copied());
    for key in options.env_remove {
        command.env_remove(key);
    }
    if let Some(current_dir) = options.current_dir {
        command.current_dir(current_dir);
    }
    let mut child = command.spawn()?;
    #[cfg(unix)]
    let process_group_id = child.id();
    let mut stdin_task = match (stdin, child.stdin.take()) {
        (Some(input), Some(mut child_stdin)) => {
            let input = input.to_vec();
            Some(tokio::spawn(async move {
                match child_stdin.write_all(&input).await {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                    Err(error) => Err(error),
                }
            }))
        }
        (Some(_), None) => return Err(io::Error::other("child stdin was not piped")),
        (None, _) => None,
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr was not piped"))?;
    let mut stdout_task = tokio::spawn(drain_bounded(stdout, options.max_stdout_bytes));
    let mut stderr_task = tokio::spawn(drain_bounded(stderr, options.max_stderr_bytes));

    let completed = tokio::time::timeout(options.timeout, async {
        let status = child.wait().await?;
        if let Some(task) = stdin_task.as_mut() {
            task.await
                .map_err(|error| io::Error::other(format!("stdin task failed: {error}")))??;
        }
        let (stdout, stdout_truncated) = (&mut stdout_task)
            .await
            .map_err(|error| io::Error::other(format!("stdout task failed: {error}")))??;
        let (_stderr, stderr_truncated) = (&mut stderr_task)
            .await
            .map_err(|error| io::Error::other(format!("stderr task failed: {error}")))??;
        Ok::<_, io::Error>((status, stdout, stdout_truncated, stderr_truncated))
    })
    .await;
    let (status, stdout, stdout_truncated, stderr_truncated) = match completed {
        Ok(result) => result?,
        Err(_) => {
            #[cfg(unix)]
            if let Some(process_id) = process_group_id {
                // SAFETY: The child starts a dedicated process group with its PID as the group ID.
                // A negative PID targets that group and cannot affect this process's own group.
                unsafe {
                    libc::kill(-(process_id as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(task) = stdin_task.as_ref() {
                task.abort();
            }
            stdout_task.abort();
            stderr_task.abort();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child process timed out",
            ));
        }
    };
    Ok(BoundedOutput {
        status,
        stdout,
        truncated: stdout_truncated || stderr_truncated,
        stdout_truncated,
        stderr_truncated,
    })
}

/// Prevent short-lived console children from flashing windows in a Windows GUI
/// process. No-op on other platforms.
#[cfg(target_os = "windows")]
fn hide_tokio_window(command: &mut tokio::process::Command) {
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_tokio_window(_command: &mut tokio::process::Command) {}

#[cfg(target_os = "windows")]
fn hide_std_window(command: &mut std::process::Command) {
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn hide_std_window(_command: &mut std::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run_script(
        script: &str,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> io::Result<BoundedOutput> {
        #[cfg(target_os = "windows")]
        return bounded_headless_output(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", script],
            timeout,
            max_output_bytes,
        )
        .await;
        #[cfg(not(target_os = "windows"))]
        return bounded_headless_output("sh", &["-c", script], timeout, max_output_bytes).await;
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn no_process_host_is_selected_without_a_resolver() {
        assert!(process_host_executable().is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn internal_marker_must_be_first_argument() {
        let args = ["app.exe", "normal", INTERNAL_PROCESS_HOST_MARKER].map(OsString::from);
        assert!(internal_process_host_args(args).is_none());
    }

    // These output-assertion tests give the child a generous budget: the
    // variable on CI is PowerShell cold-start (profile-less JIT/AMSI warmup
    // can eat several seconds on a fresh Windows runner), not the code under
    // test. Only the dedicated kill-on-timeout test keeps a tight window.
    #[tokio::test]
    async fn bounded_output_preserves_output_and_exit_status() {
        #[cfg(target_os = "windows")]
        let script = "[Console]::Out.Write('stdout'); [Console]::Error.Write('stderr'); exit 23";
        #[cfg(not(target_os = "windows"))]
        let script = "printf stdout; printf stderr >&2; exit 23";
        let output = run_script(script, Duration::from_secs(60), 64)
            .await
            .unwrap();

        assert_eq!(output.status.code(), Some(23));
        assert_eq!(output.stdout, b"stdout");
        assert!(!output.truncated);
    }

    #[tokio::test]
    async fn bounded_output_reports_stdout_and_stderr_truncation() {
        #[cfg(target_os = "windows")]
        let stdout_script = "[Console]::Out.Write('overflow')";
        #[cfg(not(target_os = "windows"))]
        let stdout_script = "printf overflow";
        let stdout = run_script(stdout_script, Duration::from_secs(60), 4)
            .await
            .unwrap();
        assert_eq!(stdout.stdout, b"over");
        assert!(stdout.truncated);

        #[cfg(target_os = "windows")]
        let stderr_script = "[Console]::Out.Write('ok'); [Console]::Error.Write('overflow')";
        #[cfg(not(target_os = "windows"))]
        let stderr_script = "printf ok; printf overflow >&2";
        let stderr = run_script(stderr_script, Duration::from_secs(60), 4)
            .await
            .unwrap();
        assert_eq!(stderr.stdout, b"ok");
        assert!(stderr.truncated);
    }

    #[tokio::test]
    async fn bounded_output_times_out() {
        #[cfg(target_os = "windows")]
        let script = "Start-Sleep -Seconds 5";
        #[cfg(not(target_os = "windows"))]
        let script = "sleep 5";
        let error = run_script(script, Duration::from_millis(50), 64)
            .await
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn bounded_output_writes_stdin_while_draining_independent_stream_limits() {
        #[cfg(target_os = "windows")]
        let (program, args) = (
            "powershell.exe",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$input | ForEach-Object { [Console]::Out.Write($_) }; [Console]::Error.Write('overflow')",
            ],
        );
        #[cfg(not(target_os = "windows"))]
        let (program, args) = ("sh", vec!["-c", "cat; printf overflow >&2"]);
        let output = bounded_headless_output_with_stdin(
            program,
            &args,
            Some(b"byte-identical stdin"),
            BoundedOutputOptions {
                timeout: Duration::from_secs(5),
                max_stdout_bytes: 64,
                max_stderr_bytes: 4,
                env: &[],
                env_remove: &[],
                current_dir: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(output.stdout, b"byte-identical stdin");
        assert!(!output.stdout_truncated);
        assert!(output.stderr_truncated);
        assert!(output.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_output_timeout_kills_descendants_after_direct_child_exits() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("descendant-finished");
        let marker_text = marker.to_str().unwrap();
        assert!(!marker_text.contains('"'));
        let script = format!("(sleep 1; touch \"{marker_text}\") &");
        let result =
            bounded_headless_output("sh", &["-c", &script], Duration::from_millis(50), 64).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(!marker.exists());
    }
}
