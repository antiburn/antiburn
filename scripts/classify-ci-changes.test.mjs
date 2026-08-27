import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyChanges,
  classifyPaths,
  isPureAppReleaseChange,
  isPureEngineReleaseChange,
  listChangedFiles,
} from "./classify-ci-changes.mjs";

function json(version, extra = {}) {
  return `${JSON.stringify({ name: "antiburn", version, ...extra }, null, 2)}\n`;
}

function cargoToml(version, dependency = "1") {
  return `[package]\nname = "antiburn"\nversion = "${version}"\n\n[dependencies]\nserde = "${dependency}"\n`;
}

function cargoLock(packageName, version, dependencyVersion = "1.0.0") {
  return `version = 4\n\n[[package]]\nname = "${packageName}"\nversion = "${version}"\ndependencies = [\n "serde",\n]\n\n[[package]]\nname = "serde"\nversion = "${dependencyVersion}"\n`;
}

function reader(values) {
  return (ref, file) => values[`${ref}:${file}`];
}

test("routes documentation without compiling application code", () => {
  assert.deepEqual(classifyPaths(["README.md", "docs/runbooks/release.md"]), {
    docs_only: true,
    frontend: false,
    engine: false,
    desktop_backend: false,
    full: false,
    release_app: false,
    release_engine: false,
  });
});

test("routes engine changes through both Rust matrices", () => {
  const result = classifyPaths(["crates/antiburn-local/src/lib.rs"]);
  assert.equal(result.engine, true);
  assert.equal(result.desktop_backend, true);
  assert.equal(result.frontend, false);
});

test("routes frontend changes without Rust matrices", () => {
  const result = classifyPaths(["apps/desktop/src/App.tsx"]);
  assert.equal(result.frontend, true);
  assert.equal(result.engine, false);
  assert.equal(result.desktop_backend, false);
});

test("fails closed for workflow and unknown paths", () => {
  for (const file of [
    ".github/workflows/ci.yml",
    "unclassified.config",
    "pnpm-lock.yaml",
    "apps/desktop/src-tauri/Cargo.toml",
  ]) {
    const result = classifyPaths([file]);
    assert.equal(result.full, true);
    assert.equal(result.frontend, true);
    assert.equal(result.engine, true);
    assert.equal(result.desktop_backend, true);
  }
});

test("includes deleted paths in routing instead of silently skipping them", () => {
  let receivedArguments;
  const files = listChangedFiles("base", "head", (...arguments_) => {
    receivedArguments = arguments_;
    return "apps/desktop/src/removed.tsx\n";
  });

  assert.deepEqual(receivedArguments, ["diff", "--name-only", "base", "head"]);
  assert.equal(classifyPaths(files).frontend, true);
});

test("recognizes an app release when every executable manifest changes only version", () => {
  const files = [
    "CHANGELOG.md",
    "apps/desktop/package.json",
    "apps/desktop/src-tauri/Cargo.lock",
    "apps/desktop/src-tauri/Cargo.toml",
    "apps/desktop/src-tauri/tauri.conf.json",
  ];
  const values = {
    "base:apps/desktop/package.json": json("1.0.0", { private: true }),
    "head:apps/desktop/package.json": json("1.0.1", { private: true }),
    "base:apps/desktop/src-tauri/tauri.conf.json": json("1.0.0", {
      productName: "antiburn",
    }),
    "head:apps/desktop/src-tauri/tauri.conf.json": json("1.0.1", {
      productName: "antiburn",
    }),
    "base:apps/desktop/src-tauri/Cargo.toml": cargoToml("1.0.0"),
    "head:apps/desktop/src-tauri/Cargo.toml": cargoToml("1.0.1"),
    "base:apps/desktop/src-tauri/Cargo.lock": cargoLock("antiburn", "1.0.0"),
    "head:apps/desktop/src-tauri/Cargo.lock": cargoLock("antiburn", "1.0.1"),
  };
  assert.equal(isPureAppReleaseChange(files, reader(values)), true);
  assert.equal(classifyChanges(files, reader(values)).release_app, true);
});

test("rejects an app release that smuggles a dependency change into Cargo files", () => {
  const files = [
    "CHANGELOG.md",
    "apps/desktop/package.json",
    "apps/desktop/src-tauri/Cargo.lock",
    "apps/desktop/src-tauri/Cargo.toml",
    "apps/desktop/src-tauri/tauri.conf.json",
  ];
  const values = {
    "base:apps/desktop/package.json": json("1.0.0"),
    "head:apps/desktop/package.json": json("1.0.1"),
    "base:apps/desktop/src-tauri/tauri.conf.json": json("1.0.0"),
    "head:apps/desktop/src-tauri/tauri.conf.json": json("1.0.1"),
    "base:apps/desktop/src-tauri/Cargo.toml": cargoToml("1.0.0", "1"),
    "head:apps/desktop/src-tauri/Cargo.toml": cargoToml("1.0.1", "2"),
    "base:apps/desktop/src-tauri/Cargo.lock": cargoLock("antiburn", "1.0.0"),
    "head:apps/desktop/src-tauri/Cargo.lock": cargoLock(
      "antiburn",
      "1.0.1",
      "2.0.0",
    ),
  };
  assert.equal(isPureAppReleaseChange(files, reader(values)), false);
  assert.equal(classifyChanges(files, reader(values)).desktop_backend, true);
});

test("recognizes an engine release across both lockfiles", () => {
  const files = [
    "apps/desktop/src-tauri/Cargo.lock",
    "crates/antiburn-local/CHANGELOG.md",
    "crates/antiburn-local/Cargo.lock",
    "crates/antiburn-local/Cargo.toml",
  ];
  const values = {
    "base:crates/antiburn-local/Cargo.toml": cargoToml("1.0.0"),
    "head:crates/antiburn-local/Cargo.toml": cargoToml("1.0.1"),
    "base:crates/antiburn-local/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.0",
    ),
    "head:crates/antiburn-local/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.1",
    ),
    "base:apps/desktop/src-tauri/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.0",
    ),
    "head:apps/desktop/src-tauri/Cargo.lock": cargoLock(
      "antiburn-local",
      "1.0.1",
    ),
  };
  assert.equal(isPureEngineReleaseChange(files, reader(values)), true);
  assert.equal(classifyChanges(files, reader(values)).release_engine, true);
});
