use std::time::Duration;

use antiburn_local::platform::process::{BoundedOutputOptions, bounded_headless_output_with_stdin};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use super::CliRuntime;

const OUTPUT_LIMIT: u64 = 8192;
const DEADLINE: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    version: String,
    delivery_protocols: Vec<String>,
}

pub async fn test(runtime: &CliRuntime) -> Result<()> {
    run(runtime, DEADLINE).await
}

async fn run(runtime: &CliRuntime, deadline: Duration) -> Result<()> {
    ensure!(
        runtime.executable.is_absolute(),
        "CLI test requires an absolute path"
    );
    let output = bounded_headless_output_with_stdin(
        &runtime.executable,
        &["capabilities", "--json"],
        None,
        BoundedOutputOptions {
            timeout: deadline,
            max_stdout_bytes: OUTPUT_LIMIT as usize,
            max_stderr_bytes: 0,
            env: &[("PATH", "")],
            env_remove: &[],
            current_dir: None,
        },
    )
    .await
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::TimedOut {
            anyhow::anyhow!("CLI test timed out")
        } else {
            anyhow::Error::new(error).context("Could not complete the installed CLI test")
        }
    })?;
    ensure!(!output.stdout_truncated, "CLI test output is too large");
    ensure!(
        !output.stderr_truncated,
        "CLI test returned unexpected diagnostics"
    );
    ensure!(
        output.status.success(),
        "CLI test exited unsuccessfully ({})",
        output.status
    );
    let stdout = output.stdout;
    let capabilities: Capabilities =
        serde_json::from_slice(&stdout).context("CLI test returned invalid capabilities")?;
    ensure!(
        capabilities.version == runtime.version,
        "CLI version does not match the installed version"
    );
    ensure!(
        capabilities.delivery_protocols == runtime.delivery_protocols,
        "CLI protocols do not match the installed protocols"
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    async fn fixture(script: &str, deadline: Duration) -> Result<()> {
        let temp = tempfile::tempdir()?;
        let executable = temp.path().join("probe");
        fs::write(&executable, format!("#!/bin/sh\n{script}\n"))?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        run(
            &CliRuntime {
                executable,
                version: "0.1.0".into(),
                sha256: "fixture".into(),
                delivery_protocols: vec!["delivery-v1".into()],
            },
            deadline,
        )
        .await
    }

    #[tokio::test]
    async fn executes_capabilities_with_an_empty_path() {
        fixture("[ -z \"$PATH\" ] && [ \"$1\" = capabilities ] && [ \"$2\" = --json ] || exit 1\nprintf '%s' '{\"version\":\"0.1.0\",\"delivery_protocols\":[\"delivery-v1\"]}'", DEADLINE).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_failed_malformed_and_mismatched_responses() {
        for script in [
            "exit 2",
            "printf invalid",
            "printf '%s' '{\"version\":\"old\",\"delivery_protocols\":[\"delivery-v1\"]}'",
            "printf '%s' '{\"version\":\"0.1.0\",\"delivery_protocols\":[]}'",
            "printf diagnostics >&2",
        ] {
            assert!(fixture(script, DEADLINE).await.is_err());
        }
    }

    #[tokio::test]
    async fn bounds_output_and_execution_time() {
        let output = fixture(
            "i=0; while [ \"$i\" -lt 900 ]; do printf '0123456789'; i=$((i+1)); done",
            DEADLINE,
        )
        .await
        .unwrap_err();
        assert!(output.to_string().contains("too large"));
        let timeout = fixture("while :; do :; done", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(timeout.to_string().contains("timed out"));
    }
}
