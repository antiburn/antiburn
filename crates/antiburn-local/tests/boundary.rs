// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Mechanical source-boundary checks for the engine.
//!
//! antiburn is local in one exact sense: it needs no connection to any
//! service of ours. As the reader's own agent it may make network
//! requests, read the credential and configuration files the
//! reader's own tools wrote, and call a provider's API with the reader's own
//! credentials. The engine's boundary is therefore not network-freeness; it
//! is two things: no data exfiltration (no telemetry/analytics SDK, no
//! reporting endpoint, no antiburn-operated or third-party host) and no
//! private/commercial provenance (no proprietary Cadence source, no
//! commercial identities, no secrets). These tests enforce that contract
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
/// (commercial identities, telemetry, publication/upload contracts).
const FORBIDDEN_ANY_CASE: &[&str] = &[
    "cadence",
    "teamcadence",
    "sentry",
    "litellm",
    "publication",
    "stripe",
    "cloudflare",
    "targetorgid",
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
    }
    assert!(
        violations.is_empty(),
        "prohibited concepts found in engine sources:\n{}",
        violations.join("\n")
    );
}

/// Dependency names that must never appear in the engine's lockfile:
/// telemetry, analytics, and crash-reporting SDKs — the "phones home"
/// capability the no-data-exfiltration line prohibits, regardless of how
/// benign the rest of a crate's networking is.
const FORBIDDEN_PACKAGES: &[&str] = &["sentry", "opentelemetry", "posthog", "segment", "datadog"];

#[test]
fn engine_lockfile_has_no_telemetry_sdk_dependencies() {
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
        "forbidden telemetry/analytics dependencies in engine lockfile: {hits:?}"
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
        let value: toml::Table = content.parse().expect("manifest parses as TOML");
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
