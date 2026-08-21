// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::{Path, PathBuf};

use antiburn_trace::{TraceConfig, init};

const LOG_DIRECTORY_NAME: &str = "antiburn-test";
const TEST_NAME: &str = "init_writes_an_event_and_a_second_call_is_inert";
const CHILD_ENV: &str = "ANTIBURN_TRACE_CHILD";
const BASE_ENV: &str = "ANTIBURN_TRACE_TEST_BASE";

#[test]
fn init_writes_an_event_and_a_second_call_is_inert() {
    match std::env::var(CHILD_ENV).as_deref() {
        Ok("normal") => {
            assert_init_and_recovered_panic();
            return;
        }
        Ok("degrade") => {
            assert_degrades_to_stderr();
            return;
        }
        Ok("env_filter") => {
            assert_env_filter_keeps_bootstrap_event();
            return;
        }
        Ok("env_filter_off") => {
            assert_env_filter_honors_trace_off();
            return;
        }
        _ => {}
    }

    let temp = tempfile::tempdir().unwrap();
    run_child("normal", &temp.path().join("normal"), None);

    let blocked = temp.path().join("blocked");
    std::fs::write(&blocked, "content").unwrap();
    run_child("degrade", &blocked, None);

    run_child(
        "env_filter",
        &temp.path().join("env-filter"),
        Some("antiburn_local=trace"),
    );
    run_child(
        "env_filter_off",
        &temp.path().join("env-filter-off"),
        Some("antiburn_trace=off"),
    );
}

fn run_child(mode: &str, base: &Path, rust_log: Option<&str>) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST_NAME])
        .env(CHILD_ENV, mode)
        .env(BASE_ENV, base)
        .env_remove("RUST_LOG");
    if cfg!(target_os = "macos") {
        command.env("HOME", base);
    } else if cfg!(target_os = "windows") {
        command.env("LOCALAPPDATA", base);
    } else {
        command.env("XDG_STATE_HOME", base);
    }
    if let Some(rust_log) = rust_log {
        command.env("RUST_LOG", rust_log);
    }

    assert!(command.status().unwrap().success());
}

fn assert_init_and_recovered_panic() {
    let expected_dir = test_log_dir();
    let config = TraceConfig {
        log_directory_name: LOG_DIRECTORY_NAME,
        debug_build: true,
    };

    let mut guard = init(&config);
    assert_eq!(guard.log_dir.as_deref(), Some(expected_dir.as_path()));
    ::tracing::error!(event = "integration_test_event");
    assert!(std::panic::catch_unwind(|| panic!("recovered test panic")).is_err());
    ::tracing::error!(event = "after_recovered_panic");
    guard.flush();
    assert_eq!(guard.log_dir.as_deref(), Some(expected_dir.as_path()));

    let contents = matching_log_contents(&expected_dir);
    assert!(contents.contains("integration_test_event"));
    assert!(contents.contains("recovered test panic"));
    assert!(contents.contains("after_recovered_panic"));

    assert!(std::panic::catch_unwind(|| panic!("panic after flush one")).is_err());
    ::tracing::error!(event = "event_after_flush");
    assert!(std::panic::catch_unwind(|| panic!("panic after flush two")).is_err());
    let contents = matching_log_contents(&expected_dir);
    assert!(!contents.contains("event_after_flush"));

    let second = init(&config);
    assert!(second.log_dir.is_none());
}

fn assert_degrades_to_stderr() {
    let guard = init(&TraceConfig {
        log_directory_name: LOG_DIRECTORY_NAME,
        debug_build: true,
    });
    assert!(guard.log_dir.is_none());
}

fn assert_env_filter_keeps_bootstrap_event() {
    let expected_dir = test_log_dir();
    let mut guard = init(&TraceConfig {
        log_directory_name: LOG_DIRECTORY_NAME,
        debug_build: true,
    });
    guard.flush();

    let contents = matching_log_contents(&expected_dir);
    assert!(contents.contains("tracing_initialized"));
    assert!(contents.contains("\"filter_source\":\"env\""));
}

fn assert_env_filter_honors_trace_off() {
    let expected_dir = test_log_dir();
    let mut guard = init(&TraceConfig {
        log_directory_name: LOG_DIRECTORY_NAME,
        debug_build: true,
    });
    assert_eq!(guard.log_dir.as_deref(), Some(expected_dir.as_path()));
    guard.flush();

    let contents = matching_log_contents(&expected_dir);
    assert!(!contents.contains("tracing_initialized"));
}

fn test_log_dir() -> PathBuf {
    let base = PathBuf::from(std::env::var_os(BASE_ENV).unwrap());
    if cfg!(target_os = "macos") {
        base.join("Library/Logs").join(LOG_DIRECTORY_NAME)
    } else {
        base.join(LOG_DIRECTORY_NAME).join("logs")
    }
}

fn matching_log_contents(log_dir: &Path) -> String {
    std::fs::read_dir(log_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("antiburn.") && name.ends_with(".log")
        })
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
        .collect()
}
