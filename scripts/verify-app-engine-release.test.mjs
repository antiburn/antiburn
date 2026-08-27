import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { verifyAppEngineRelease } from "./verify-app-engine-release.mjs";

function git(root, ...arguments_) {
  return execFileSync("git", arguments_, {
    cwd: root,
    encoding: "utf8",
  }).trim();
}

function writeFixture(root, engineVersion, lockedVersion = engineVersion) {
  mkdirSync(path.join(root, "crates/antiburn-local/src"), { recursive: true });
  mkdirSync(path.join(root, "apps/desktop/src-tauri"), { recursive: true });
  writeFileSync(
    path.join(root, "crates/antiburn-local/Cargo.toml"),
    `[package]\nname = "antiburn-local"\nversion = "${engineVersion}"\n`,
  );
  writeFileSync(
    path.join(root, "crates/antiburn-local/src/lib.rs"),
    "pub fn released() {}\n",
  );
  writeFileSync(
    path.join(root, "apps/desktop/src-tauri/Cargo.lock"),
    `version = 4\n\n[[package]]\nname = "antiburn-local"\nversion = "${lockedVersion}"\n`,
  );
}

function repository(t, options = {}) {
  const root = mkdtempSync(path.join(tmpdir(), "antiburn-engine-release-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  git(root, "init", "--quiet");
  git(root, "config", "user.name", "Release Test");
  git(root, "config", "user.email", "release-test@example.com");
  writeFixture(root, "1.2.3", options.lockedVersion);
  git(root, "add", ".");
  git(root, "commit", "--quiet", "-m", "engine source");
  if (options.tag === "annotated") {
    git(root, "tag", "-a", "antiburn-local-v1.2.3", "-m", "engine 1.2.3");
  } else if (options.tag === "lightweight") {
    git(root, "tag", "antiburn-local-v1.2.3");
  }
  return root;
}

test("accepts an exact engine tree from an annotated ancestral tag", (t) => {
  const root = repository(t, { tag: "annotated" });
  const result = verifyAppEngineRelease(root);

  assert.deepEqual(result.errors, []);
  assert.equal(result.engineVersion, "1.2.3");
  assert.equal(result.tag, "antiburn-local-v1.2.3");
  assert.equal(result.currentTree, result.releasedTree);
});

test("rejects engine changes made after the matching release tag", (t) => {
  const root = repository(t, { tag: "annotated" });
  writeFileSync(
    path.join(root, "crates/antiburn-local/src/lib.rs"),
    "pub fn unreleased() {}\n",
  );
  git(root, "add", ".");
  git(root, "commit", "--quiet", "-m", "unreleased engine change");

  const result = verifyAppEngineRelease(root);

  assert.match(
    result.errors.join("\n"),
    /does not match antiburn-local-v1\.2\.3/,
  );
  assert.match(
    result.errors.join("\n"),
    /crates\/antiburn-local\/src\/lib\.rs/,
  );
});

test("rejects a missing engine release tag", (t) => {
  const root = repository(t);
  const result = verifyAppEngineRelease(root);

  assert.match(
    result.errors.join("\n"),
    /antiburn-local-v1\.2\.3 does not exist/,
  );
});

test("rejects a lightweight engine tag", (t) => {
  const root = repository(t, { tag: "lightweight" });
  const result = verifyAppEngineRelease(root);

  assert.match(result.errors.join("\n"), /is not an annotated tag/);
});

test("rejects a stale engine version in the desktop lockfile", (t) => {
  const root = repository(t, { tag: "annotated", lockedVersion: "1.2.2" });
  const result = verifyAppEngineRelease(root);

  assert.match(
    result.errors.join("\n"),
    /Cargo\.lock records antiburn-local 1\.2\.2.*declares 1\.2\.3/,
  );
});
