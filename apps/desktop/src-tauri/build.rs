include!("src/app_commands.rs");

use std::env;
use std::fs;

macro_rules! command_names {
    ($( $handler:path => $name:literal, )*) => {
        &[$($name),*]
    };
}

const APP_COMMANDS: &[&str] = with_app_commands!(command_names);

fn command_permission_sources() -> Vec<String> {
    ["capabilities", "permissions"]
        .into_iter()
        .flat_map(|directory| {
            fs::read_dir(directory)
                .unwrap_or_else(|error| panic!("failed to read {directory}: {error}"))
        })
        .filter_map(|entry| {
            let path = entry
                .expect("failed to read a permission source path")
                .path();
            path.is_file().then_some(path)
        })
        .map(|path| {
            fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
        })
        .collect()
}

fn contains_quoted_value(source: &str, expected: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .any(|line| {
            line.split('"')
                .skip(1)
                .step_by(2)
                .any(|value| value == expected)
        })
}

fn validate_command_permissions() {
    let sources = command_permission_sources();
    let missing = APP_COMMANDS
        .iter()
        .filter_map(|command| {
            let permission = format!("allow-{}", command.replace('_', "-"));
            (!sources
                .iter()
                .any(|source| contains_quoted_value(source, &permission)))
            .then_some(*command)
        })
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "registered Tauri commands have no manual permission set: {}",
        missing.join(", ")
    );
}

fn configure_windows_common_controls() {
    let is_windows_msvc = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if is_windows_msvc {
        // Tauri manifests the app binary, but Cargo test binaries also need Common-Controls v6.
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}

fn main() {
    println!(
        "cargo:rustc-env=ANTIBURN_CLI_TARGET={}",
        env::var("TARGET").expect("Cargo target")
    );
    // `analytics::config` reads these with `option_env!`, which is
    // resolved at compile time. Cargo does not track an environment variable
    // as an input unless it is told to, so without these two lines changing
    // the endpoint would leave a stale binary behind and the change would
    // look like it had no effect — the worst kind of build bug, because it
    // reads as a code problem.
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_URL");
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_OPERATOR");
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_ENABLED");
    println!("cargo:rerun-if-env-changed=GOOGLE_ANTIGRAVITY_2_IDE_AGY_OAUTH_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=GOOGLE_ANTIGRAVITY_2_IDE_AGY_OAUTH_CLIENT_SECRET");
    println!("cargo:rerun-if-changed=capabilities");
    println!("cargo:rerun-if-changed=permissions");
    validate_command_permissions();
    configure_windows_common_controls();
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(APP_COMMANDS));
    tauri_build::try_build(attributes).expect("failed to build the Tauri application");
}
