// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Mechanical source-boundary checks for the engine.
//!
//! The engine is the canonical, network-free local implementation: no
//! private/commercial concepts, no network APIs, no live-probe behavior,
//! and no network-capable dependencies. These tests enforce that contract
//! mechanically so a violation fails CI instead of relying on review.
//!
//! The approved source manifests (`source-allowlist.toml` /
//! `source-denylist.toml`) ship with the repository; this suite validates
//! them and derives its prohibited-concept list from the denylist's concept
//! rules. The manifest files themselves are exempt from concept-string
//! matching (their rule text names the concepts), as is this file (its
//! pattern table names them too).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Directory holding the approved source manifests. The public repository
/// ships them at `docs/oss/`; the private monorepo vendors them at
/// `docs/desktop/oss/`. Overridable for other layouts.
fn oss_manifest_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ANTIBURN_OSS_MANIFEST_DIR") {
        return PathBuf::from(dir);
    }
    let public = manifest_dir().join("../../docs/oss");
    if public.join("source-allowlist.toml").exists() {
        return public;
    }
    manifest_dir().join("../../docs/desktop/oss")
}

/// Every text file under the engine's `src/` and `tests/` trees.
fn engine_text_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in ["src", "tests"] {
        collect_files(&manifest_dir().join(root), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 40,
        "expected the full engine tree, found only {} files",
        files.len()
    );
    files
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "md" | "json" | "jsonl" | "toml" | "css" | "ts" | "tsx")
        ) {
            out.push(path);
        }
    }
}

/// This file names the prohibited tokens in its pattern tables, so it must
/// not scan itself.
fn is_exempt(path: &Path) -> bool {
    path.ends_with("tests/boundary.rs")
}

/// Case-insensitive concept tokens derived from the denylist's concept rules
/// (commercial identities, telemetry, publication/upload contracts, live
/// agent probes, provider-credential access).
const FORBIDDEN_ANY_CASE: &[&str] = &[
    "cadence",
    "teamcadence",
    "sentry",
    "litellm",
    "publication",
    "stripe",
    "cloudflare",
    "csrf",
    "lsof",
    "netstat",
    "targetorgid",
];

/// Case-sensitive code-level tokens: network APIs and live-probe endpoints
/// prohibited inside the engine. Filesystem path forms of "localhost"
/// (`\\wsl.localhost\…`, `file://localhost/…`) are legitimate and excluded
/// by anchoring the URL schemes.
const FORBIDDEN_CODE: &[&str] = &[
    "reqwest",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "UnixStream",
    "UnixListener",
    "http://localhost",
    "https://localhost",
    "127.0.0.1",
];

#[test]
fn engine_sources_contain_no_prohibited_concepts() {
    let mut violations = Vec::new();
    for path in engine_text_files() {
        if is_exempt(&path) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let lower = content.to_lowercase();
        for token in FORBIDDEN_ANY_CASE {
            if lower.contains(token) {
                violations.push(format!("{}: contains {token:?}", path.display()));
            }
        }
        for token in FORBIDDEN_CODE {
            if content.contains(token) {
                violations.push(format!("{}: contains {token:?}", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "prohibited concepts found in engine sources:\n{}",
        violations.join("\n")
    );
}

/// Dependency names that must never appear in the engine's lockfile:
/// HTTP/TLS stacks, telemetry SDKs, and app-shell frameworks.
const FORBIDDEN_PACKAGES: &[&str] = &[
    "reqwest",
    "hyper",
    "ureq",
    "curl",
    "isahc",
    "sentry",
    "tauri",
    "tungstenite",
    "native-tls",
    "openssl",
    "rustls",
    "quinn",
    "axum",
    "actix-web",
    "tonic",
];

#[test]
fn engine_lockfile_has_no_network_capable_dependencies() {
    let lock = fs::read_to_string(manifest_dir().join("Cargo.lock")).expect("engine Cargo.lock");
    let names: BTreeSet<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .collect();
    let hits: Vec<&&str> = FORBIDDEN_PACKAGES
        .iter()
        .filter(|p| names.contains(**p))
        .collect();
    assert!(
        hits.is_empty(),
        "forbidden dependencies in engine lockfile: {hits:?}"
    );
}

#[test]
fn tokio_never_enables_the_net_feature() {
    let manifest = fs::read_to_string(manifest_dir().join("Cargo.toml")).expect("Cargo.toml");
    let value: toml::Value = manifest.parse().expect("Cargo.toml parses");
    let tokio = value
        .get("dependencies")
        .and_then(|d| d.get("tokio"))
        .expect("tokio dependency present");
    let features: Vec<&str> = tokio
        .get("features")
        .and_then(|f| f.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        !features.contains(&"net") && !features.contains(&"full"),
        "tokio must not enable network features; found {features:?}"
    );
}

#[test]
fn approved_source_manifests_are_present_and_well_formed() {
    let dir = oss_manifest_dir();
    for name in ["source-allowlist.toml", "source-denylist.toml"] {
        let path = dir.join(name);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{} must ship with the repository (set ANTIBURN_OSS_MANIFEST_DIR \
                 if relocated): {e}",
                path.display()
            )
        });
        let value: toml::Value = content.parse().expect("manifest parses as TOML");
        assert_eq!(
            value.get("schema_version").and_then(|v| v.as_integer()),
            Some(1),
            "{name}: schema_version"
        );
        assert_eq!(
            value.get("status").and_then(|v| v.as_str()),
            Some("approved"),
            "{name}: status"
        );
        let rules = value
            .get("rule")
            .and_then(|r| r.as_array())
            .expect("manifest has [[rule]] entries");
        let mut ids = BTreeSet::new();
        for rule in rules {
            let id = rule
                .get("id")
                .and_then(|v| v.as_str())
                .expect("every rule has an id");
            assert!(ids.insert(id.to_string()), "{name}: duplicate rule id {id}");
        }
        assert!(!ids.is_empty(), "{name}: no rules parsed");
    }
}
