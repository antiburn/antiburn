mod lifecycle;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use lifecycle::{
    CliInstallState, CliRuntime, Installation, Manifest, RuntimeDigestCache, file_name,
};

pub const TARGET: &str = env!("ANTIBURN_CLI_RUNTIME_TARGET");
