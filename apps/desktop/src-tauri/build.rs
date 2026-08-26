// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

fn main() {
    // `analytics::config` reads these with `option_env!`, which is
    // resolved at compile time. Cargo does not track an environment variable
    // as an input unless it is told to, so without these two lines changing
    // the endpoint would leave a stale binary behind and the change would
    // look like it had no effect — the worst kind of build bug, because it
    // reads as a code problem.
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_URL");
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_OPERATOR");
    build_foundation_model_sidecar();
    tauri_build::build();
}

/// Compile the macOS run-foundation-model sidecar before Tauri validates the
/// bundle. The Swift source guards itself: an SDK without FoundationModels
/// still compiles, as a stub that always reports unavailable.
fn build_foundation_model_sidecar() {
    println!("cargo:rerun-if-changed=sidecar/run-foundation-model.swift");
    let target = std::env::var("TARGET").unwrap_or_default();
    // The runtime lookup in `crate::titles` uses the same triple.
    println!("cargo:rustc-env=ANTIBURN_TARGET_TRIPLE={target}");
    if !target.ends_with("-apple-darwin") {
        return;
    }
    let arch = match target.split('-').next() {
        Some("aarch64") => "arm64",
        Some(arch) => arch,
        None => return,
    };
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = manifest.join("binaries");
    std::fs::create_dir_all(&out).expect("create the sidecar output directory");
    let status = std::process::Command::new("swiftc")
        .arg("-O")
        .arg(manifest.join("sidecar/run-foundation-model.swift"))
        .arg("-o")
        .arg(out.join(format!("run-foundation-model-{target}")))
        // Match the app's minimumSystemVersion in tauri.conf.json.
        .arg("-target")
        .arg(format!("{arch}-apple-macos13.0"))
        .status()
        .expect("run swiftc for the run-foundation-model sidecar");
    assert!(
        status.success(),
        "swiftc failed for the run-foundation-model sidecar"
    );
}
