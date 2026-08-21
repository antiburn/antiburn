// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local diagnostic output for the antiburn desktop shell.
//!
//! The crate writes compact events to stderr and JSON events to hourly local
//! files. File writes stay synchronous because the default event volume is low.

mod paths;
mod retention;
mod writer;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use writer::HourlyFileWriter;

pub const LOG_FILE_PREFIX: &str = "antiburn";
pub const DEFAULT_LOG_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const DEBUG_FILTER: &str = "info,antiburn_desktop=debug,antiburn_local=debug,antiburn_nudge=debug,antiburn_sound=debug,antiburn_trace=debug";
const RELEASE_FILTER: &str = "info";
const TRACE_TARGET: &str = "antiburn_trace";

static INIT_LOCK: Mutex<()> = Mutex::new(());
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct TraceConfig<'a> {
    /// The directory name selects the log directory.
    pub log_directory_name: &'a str,
    /// A debug build uses the more detailed default filter.
    pub debug_build: bool,
}

#[must_use = "retain the guard while file tracing must remain active"]
pub struct TraceGuard {
    writer: Option<HourlyFileWriter>,
    /// The log directory selected when file output starts.
    ///
    /// This value stays set after [`Self::flush`].
    pub log_dir: Option<PathBuf>,
}

impl TraceGuard {
    /// Flush and stop the file writer. This operation is idempotent.
    pub fn flush(&mut self) {
        if let Some(writer) = self.writer.take() {
            let _ = writer.stop();
        }
    }

    fn inert() -> Self {
        Self {
            writer: None,
            log_dir: None,
        }
    }
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Install local outputs and the panic hook.
///
/// The function uses stderr when the file directory is not available. A second
/// call returns an inert guard and does not change the installed subscriber.
pub fn init(config: &TraceConfig<'_>) -> TraceGuard {
    let _init = INIT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if INSTALLED.load(std::sync::atomic::Ordering::Acquire) {
        return TraceGuard::inert();
    }

    let (filter, filter_source) = filter(config.debug_build);
    let filter_text = filter.to_string();
    let resolved_dir = paths::resolve_log_dir(config.log_directory_name);
    let writer = resolved_dir
        .as_deref()
        .and_then(|dir| HourlyFileWriter::new(dir).ok());
    let file_unavailable = writer.is_none();

    let stderr_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_ansi(config.debug_build);
    let (installed, writer) = match writer {
        Some(writer) => {
            let file_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .with_writer(writer.clone());
            let installed = tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .with(file_layer)
                .try_init()
                .is_ok();
            (installed, installed.then_some(writer))
        }
        None => {
            let installed = tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .try_init()
                .is_ok();
            (installed, None)
        }
    };

    if !installed {
        return TraceGuard::inert();
    }
    INSTALLED.store(true, std::sync::atomic::Ordering::Release);
    install_panic_hook(writer.clone());

    if file_unavailable {
        ::tracing::warn!(event = "log_dir_unavailable");
    }
    let log_dir = writer.as_ref().and(resolved_dir);
    let build = if config.debug_build {
        "debug"
    } else {
        "release"
    };
    ::tracing::info!(
        event = "tracing_initialized",
        log_dir = ?log_dir,
        build,
        filter = %filter_text,
        filter_source
    );

    TraceGuard { writer, log_dir }
}

fn filter(debug_build: bool) -> (EnvFilter, &'static str) {
    match EnvFilter::try_from_default_env() {
        Ok(filter) => {
            let filter = match std::env::var("RUST_LOG") {
                Ok(value) if env_controls_trace_bootstrap(&value) => filter,
                _ => filter.add_directive(
                    "antiburn_trace=info"
                        .parse()
                        .expect("the bootstrap filter must be valid"),
                ),
            };
            (filter, "env")
        }
        Err(_) => {
            let fallback = if debug_build {
                DEBUG_FILTER
            } else {
                RELEASE_FILTER
            };
            (
                EnvFilter::try_new(fallback).expect("the built-in filter must be valid"),
                "default",
            )
        }
    }
}

fn env_controls_trace_bootstrap(value: &str) -> bool {
    value.split(',').any(|directive| {
        let directive = directive.trim();
        let level = directive.to_ascii_lowercase();
        matches!(
            level.as_str(),
            "off" | "error" | "warn" | "info" | "debug" | "trace"
        ) || directive
            .strip_prefix(TRACE_TARGET)
            .is_some_and(|rest| rest.starts_with('=') || rest.starts_with('['))
    })
}

fn install_panic_hook(writer: Option<HourlyFileWriter>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let message = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic payload is not text");
        let location = panic
            .location()
            .map(ToString::to_string)
            .unwrap_or_else(|| "unknown".to_owned());
        ::tracing::error!(event = "panic", message, location);
        if let Some(writer) = &writer {
            let _ = writer.flush();
        }
        previous(panic);
    }));
}

/// Delete matching rotated files older than `max_age`.
///
/// Only regular files named `antiburn.*.log` are candidates. The operation is
/// synchronous so the caller selects the thread.
pub fn clean_old_logs(log_dir: &Path, max_age: Duration) -> std::io::Result<usize> {
    retention::clean_old_logs(log_dir, max_age)
}

#[cfg(test)]
mod tests {
    use super::env_controls_trace_bootstrap;

    #[test]
    fn explicit_trace_or_global_filters_control_bootstrap_events() {
        assert!(!env_controls_trace_bootstrap("antiburn_local=trace"));
        assert!(env_controls_trace_bootstrap("antiburn_trace=off"));
        assert!(env_controls_trace_bootstrap("warn,antiburn_local=trace"));
    }
}
