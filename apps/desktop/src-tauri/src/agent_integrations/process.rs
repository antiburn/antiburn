use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    Unavailable,
    Timeout,
    OutputLimit,
    Failed,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn hanging_and_unbounded_probes_are_stopped_within_limits() {
        let env = BTreeMap::new();
        let started = Instant::now();
        assert_eq!(
            NativeRunner.run(
                Path::new("/bin/sh"),
                &["-c".into(), "/bin/sleep 30".into()],
                &env,
                Duration::from_millis(50)
            ),
            Err(ProcessError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            NativeRunner.run(Path::new("/usr/bin/yes"), &[], &env, PROBE_TIMEOUT),
            Err(ProcessError::OutputLimit)
        );
        assert_eq!(
            NativeRunner.run(
                Path::new("/bin/sh"),
                &["-c".into(), "while :; do printf overflow >&2; done".into()],
                &env,
                Duration::from_millis(50),
            ),
            Err(ProcessError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn successful_probe_does_not_leave_background_helpers_running() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("marker");
        let script = "(/bin/sleep 0.2; printf leaked > \"$1\") & printf ok";
        assert_eq!(
            NativeRunner
                .run(
                    Path::new("/bin/sh"),
                    &[
                        "-c".into(),
                        script.into(),
                        "probe".into(),
                        marker.to_string_lossy().into_owned()
                    ],
                    &BTreeMap::new(),
                    PROBE_TIMEOUT
                )
                .unwrap(),
            "ok"
        );
        std::thread::sleep(Duration::from_millis(400));
        assert!(!marker.exists());
    }

    #[test]
    fn probes_only_receive_explicit_environment_without_node_injection() {
        let env = BTreeMap::from([
            ("ALLOWED_VALUE".into(), "accepted".into()),
            ("NODE_OPTIONS".into(), "--require untrusted".into()),
            ("NODE_PATH".into(), "/untrusted".into()),
        ]);
        assert_eq!(
            NativeRunner
                .run(Path::new("/usr/bin/env"), &[], &env, PROBE_TIMEOUT)
                .unwrap(),
            "ALLOWED_VALUE=accepted\n"
        );
        assert_eq!(
            NativeRunner
                .run(Path::new("/bin/pwd"), &[], &BTreeMap::new(), PROBE_TIMEOUT)
                .unwrap(),
            "/\n"
        );
    }

    #[test]
    fn probes_preserve_exact_stdout_limit_and_ignore_stderr() {
        let run = |script: &str| {
            NativeRunner.run(
                Path::new("/bin/sh"),
                &["-c".into(), script.into()],
                &BTreeMap::new(),
                PROBE_TIMEOUT,
            )
        };
        assert_eq!(run("printf '%65536s' ''").unwrap().len(), MAX_OUTPUT);
        assert_eq!(run("printf '%65537s' ''"), Err(ProcessError::OutputLimit));
        assert_eq!(run("printf ignored >&2; printf ok").unwrap(), "ok");
        assert_eq!(run("printf '\\377'"), Err(ProcessError::Failed));
        assert_eq!(run("exit 23"), Err(ProcessError::Failed));
    }

    #[test]
    fn probe_timeout_stops_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("marker");
        assert_eq!(
            NativeRunner.run(
                Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "(/bin/sleep 0.3; printf leaked > \"$1\") & wait".into(),
                    "probe".into(),
                    marker.to_string_lossy().into_owned(),
                ],
                &BTreeMap::new(),
                Duration::from_millis(50),
            ),
            Err(ProcessError::Timeout)
        );
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists());
    }

    #[test]
    fn probes_reject_relative_paths_and_unbounded_deadlines() {
        assert_eq!(
            NativeRunner.run(Path::new("sh"), &[], &BTreeMap::new(), PROBE_TIMEOUT),
            Err(ProcessError::Unavailable)
        );
        assert_eq!(
            NativeRunner.run(
                Path::new("/bin/sh"),
                &[],
                &BTreeMap::new(),
                Duration::from_secs(31),
            ),
            Err(ProcessError::Unavailable)
        );
    }

    #[tokio::test]
    async fn synchronous_probes_also_work_inside_a_tokio_runtime() {
        assert_eq!(
            NativeRunner
                .run(
                    Path::new("/bin/sh"),
                    &["-c".into(), "printf ok".into()],
                    &BTreeMap::new(),
                    PROBE_TIMEOUT,
                )
                .unwrap(),
            "ok"
        );
    }
}

pub trait Runner {
    fn run(
        &self,
        executable: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<String, ProcessError>;
}

pub struct NativeRunner;

impl Runner for NativeRunner {
    #[cfg(unix)]
    fn run(
        &self,
        executable: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> Result<String, ProcessError> {
        use antiburn_local::platform::process::{
            BoundedOutputOptions, BoundedOutputPolicy,
            bounded_headless_output_with_stdin_and_policy,
        };

        if !executable.is_absolute() || timeout > Duration::from_secs(30) {
            return Err(ProcessError::Unavailable);
        }
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let env = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        // Use a separate thread so this synchronous call can also run inside Tokio.
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .spawn_scoped(scope, || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| ProcessError::Unavailable)?;
                    let output = runtime
                        .block_on(bounded_headless_output_with_stdin_and_policy(
                            executable,
                            &args,
                            None,
                            BoundedOutputOptions {
                                timeout,
                                max_stdout_bytes: MAX_OUTPUT,
                                max_stderr_bytes: 0,
                                env: &env,
                                env_remove: &["NODE_OPTIONS", "NODE_PATH"],
                                current_dir: Some(Path::new("/")),
                            },
                            BoundedOutputPolicy {
                                clear_env: true,
                                fail_on_stdout_limit: true,
                                kill_descendants_on_exit: true,
                            },
                        ))
                        .map_err(|error| match error.kind() {
                            std::io::ErrorKind::TimedOut => ProcessError::Timeout,
                            std::io::ErrorKind::FileTooLarge => ProcessError::OutputLimit,
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                                ProcessError::Unavailable
                            }
                            _ => ProcessError::Failed,
                        })?;
                    if !output.status.success() {
                        return Err(ProcessError::Failed);
                    }
                    String::from_utf8(output.stdout).map_err(|_| ProcessError::Failed)
                })
                .map_err(|_| ProcessError::Unavailable)?
                .join()
                .map_err(|_| ProcessError::Failed)?
        })
    }

    #[cfg(not(unix))]
    fn run(
        &self,
        _: &Path,
        _: &[String],
        _: &BTreeMap<String, String>,
        _: Duration,
    ) -> Result<String, ProcessError> {
        Err(ProcessError::Unavailable)
    }
}
