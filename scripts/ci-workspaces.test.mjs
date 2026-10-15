// These tests compare the workspace registry with the Rust sources. A Rust
// change can add a path dependency or a doctest without a change to the
// registry, so the classify job runs this file on every event.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

import { WORKSPACES, owningWorkspace } from "./ci-workspaces.mjs";

const ROOT = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
}).trim();

function trackedFiles(...patterns) {
  return execFileSync("git", ["ls-files", "--", ...patterns], {
    cwd: ROOT,
    encoding: "utf8",
  })
    .split("\n")
    .filter(Boolean);
}

/** The registry ids that the `path = "..."` entries of a manifest reach. */
function pathDependencies(workspace) {
  const manifest = readFileSync(
    path.join(ROOT, workspace.directory, "Cargo.toml"),
    "utf8",
  );
  const ids = new Set();
  for (const [, relative] of manifest.matchAll(/\bpath\s*=\s*"([^"]+)"/g)) {
    const target = path.posix.normalize(
      path.posix.join(workspace.directory, relative),
    );
    // Target paths such as src/lib.rs stay inside the workspace.
    const owner = owningWorkspace(`${target}/`);
    assert.ok(
      owner,
      `${workspace.id}: ${relative} is not a registry workspace`,
    );
    if (owner.id !== workspace.id) ids.add(owner.id);
  }
  return [...ids].sort();
}

test("dependsOn matches the path dependencies in each Cargo.toml", () => {
  for (const workspace of WORKSPACES) {
    assert.deepEqual(
      [...workspace.dependsOn].sort(),
      pathDependencies(workspace),
      `${workspace.id}: update dependsOn in scripts/ci-workspaces.mjs`,
    );
  }
});

// Rustdoc tests a fenced block that has no language, or only Rust
// attributes. It skips `ignore` blocks and blocks in other languages.
const RUST_FENCE_ATTRIBUTES = new Set([
  "rust",
  "no_run",
  "should_panic",
  "compile_fail",
  "test_harness",
  "standalone_crate",
]);

function isDoctestFence(info) {
  const tags = info.split(/[\s,]+/).filter(Boolean);
  if (tags.includes("ignore")) return false;
  return tags.every(
    (tag) => RUST_FENCE_ATTRIBUTES.has(tag) || /^edition\d{4}$/.test(tag),
  );
}

/** The `file:line` of each doctest in the library sources of a workspace. */
function doctests(workspace) {
  const found = [];
  const sources = trackedFiles(`${workspace.directory}/src/*.rs`).filter(
    (file) => owningWorkspace(file).id === workspace.id,
  );
  for (const source of sources) {
    const lines = readFileSync(path.join(ROOT, source), "utf8").split("\n");
    let open = false;
    lines.forEach((line, index) => {
      assert.doesNotMatch(
        line,
        /#!?\[doc\s*=\s*include_str!/,
        `${source}:${index + 1}: extend this scan to read the included file`,
      );
      const doc = /^\s*\/\/[/!]\s?(.*)$/.exec(line);
      if (doc === null) {
        open = false;
        return;
      }
      const fence = /^\s*```(.*)$/.exec(doc[1]);
      if (fence === null) return;
      if (!open && isDoctestFence(fence[1])) {
        found.push(`${source}:${index + 1}`);
      }
      open = !open;
    });
  }
  return found;
}

test("the doctests flag is set exactly where the library has doctests", () => {
  for (const workspace of WORKSPACES) {
    const found = doctests(workspace);
    assert.equal(
      Boolean(workspace.doctests),
      found.length > 0,
      `${workspace.id}: set doctests to ${found.length > 0} in scripts/ci-workspaces.mjs` +
        (found.length > 0 ? ` (first doctest: ${found[0]})` : ""),
    );
  }
});

test("the doctest scan reads fence languages like rustdoc", () => {
  assert.equal(isDoctestFence(""), true);
  assert.equal(isDoctestFence("rust"), true);
  assert.equal(isDoctestFence("no_run"), true);
  assert.equal(isDoctestFence("rust,should_panic"), true);
  assert.equal(isDoctestFence("edition2021"), true);
  assert.equal(isDoctestFence("ignore"), false);
  assert.equal(isDoctestFence("text"), false);
  assert.equal(isDoctestFence("json"), false);
  assert.equal(isDoctestFence("sh"), false);
});
