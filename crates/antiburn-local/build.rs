//! Stages the tool catalogue the engine embeds with `include_str!`.
//!
//! `ANTIBURN_TOOL_CATALOG` names a catalogue JSON file that
//! `scripts/build-tool-catalog.mjs` wrote from an `antiburn/systemprompts`
//! checkout. The release workflow sets it. When it is not set, the build
//! uses the small committed fixture, so `cargo build` and `cargo test` need
//! no checkout and no Node.

use std::env;
use std::fs;
use std::path::PathBuf;

const CATALOG_ENV: &str = "ANTIBURN_TOOL_CATALOG";
const FIXTURE: &str = "tests/fixtures/tool_catalog.json";

fn main() {
    println!("cargo:rerun-if-env-changed={CATALOG_ENV}");

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let source = match env::var_os(CATALOG_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => manifest_dir.join(FIXTURE),
    };
    println!("cargo:rerun-if-changed={}", source.display());

    let destination =
        PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("tool_catalog.json");
    if let Err(error) = fs::copy(&source, &destination) {
        panic!(
            "cannot copy the tool catalogue from {} to {}: {error}. \
             Set {CATALOG_ENV} to a file scripts/build-tool-catalog.mjs wrote, or unset it to use {FIXTURE}.",
            source.display(),
            destination.display()
        );
    }
}
