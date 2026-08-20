#!/usr/bin/env node
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Prove that an application release embeds an actual antiburn-local release.
//
// The desktop path-depends on the in-tree engine, so Cargo correctly compiles
// whatever source is present at the application tag. That convenience must not
// let an app release label unreleased engine source with an older crate version
// in its SBOM. The engine's declared version therefore names an annotated,
// ancestral antiburn-local tag whose complete crate tree must be byte-for-byte
// identical to the one the application is about to ship.

import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ENGINE_MANIFEST = "crates/antiburn-local/Cargo.toml";
const APP_LOCKFILE = "apps/desktop/src-tauri/Cargo.lock";
const ENGINE_TREE = "crates/antiburn-local";

function git(root, ...arguments_) {
  const result = spawnSync("git", arguments_, {
    cwd: root,
    encoding: "utf8",
  });
  return {
    status: result.status,
    stdout: result.stdout.trim(),
    stderr: result.stderr.trim(),
  };
}

function cargoTomlVersion(contents) {
  let inPackage = false;
  for (const rawLine of contents.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line.startsWith("[")) {
      inPackage = line === "[package]";
      continue;
    }
    if (!inPackage) continue;
    const match = /^version\s*=\s*"([^"]+)"/.exec(line);
    if (match) return match[1];
  }
  return null;
}

function cargoLockVersion(contents, packageName) {
  for (const block of contents.split("[[package]]")) {
    const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(block);
    if (!name || name[1] !== packageName) continue;
    return /^\s*version\s*=\s*"([^"]+)"/m.exec(block)?.[1] ?? null;
  }
  return null;
}

function show(root, ref, file) {
  return git(root, "show", `${ref}:${file}`);
}

export function verifyAppEngineRelease(root = ROOT, ref = "HEAD") {
  const errors = [];
  const manifest = show(root, ref, ENGINE_MANIFEST);
  if (manifest.status !== 0) {
    return {
      errors: [
        `${ENGINE_MANIFEST} could not be read at ${ref}: ${manifest.stderr}`,
      ],
    };
  }

  const engineVersion = cargoTomlVersion(manifest.stdout);
  if (!engineVersion) {
    return {
      errors: [`${ENGINE_MANIFEST} has no [package] version at ${ref}`],
    };
  }

  const lockfile = show(root, ref, APP_LOCKFILE);
  if (lockfile.status !== 0) {
    errors.push(
      `${APP_LOCKFILE} could not be read at ${ref}: ${lockfile.stderr}`,
    );
  } else {
    const lockedVersion = cargoLockVersion(lockfile.stdout, "antiburn-local");
    if (lockedVersion !== engineVersion) {
      errors.push(
        `${APP_LOCKFILE} records antiburn-local ${lockedVersion ?? "without a version"}, ` +
          `but ${ENGINE_MANIFEST} declares ${engineVersion}`,
      );
    }
  }

  const tag = `antiburn-local-v${engineVersion}`;
  const tagRef = `refs/tags/${tag}`;
  const objectType = git(root, "cat-file", "-t", tagRef);
  if (objectType.status !== 0) {
    errors.push(
      `${tag} does not exist. Release the changed engine before releasing the application.`,
    );
    return { engineVersion, tag, errors };
  }
  if (objectType.stdout !== "tag") {
    errors.push(`${tag} is not an annotated tag`);
  }

  const tagCommit = git(root, "rev-parse", `${tagRef}^{commit}`);
  if (tagCommit.status !== 0) {
    errors.push(`${tag} does not resolve to a commit: ${tagCommit.stderr}`);
    return { engineVersion, tag, errors };
  }

  const ancestry = git(
    root,
    "merge-base",
    "--is-ancestor",
    tagCommit.stdout,
    ref,
  );
  if (ancestry.status !== 0) {
    errors.push(`${tag} is not an ancestor of ${ref}`);
  }

  const currentTree = git(root, "rev-parse", `${ref}:${ENGINE_TREE}`);
  const releasedTree = git(
    root,
    "rev-parse",
    `${tagRef}^{commit}:${ENGINE_TREE}`,
  );
  if (currentTree.status !== 0 || releasedTree.status !== 0) {
    errors.push(`Could not compare ${ENGINE_TREE} at ${ref} and ${tag}`);
  } else if (currentTree.stdout !== releasedTree.stdout) {
    const changed = git(
      root,
      "diff",
      "--name-only",
      tagCommit.stdout,
      ref,
      "--",
      ENGINE_TREE,
    );
    const detail = changed.stdout ? ` Changed files:\n${changed.stdout}` : "";
    errors.push(
      `${ENGINE_TREE} at ${ref} does not match ${tag}. ` +
        `Release the changed engine before releasing the application.${detail}`,
    );
  }

  return {
    engineVersion,
    tag,
    tagCommit: tagCommit.stdout,
    currentTree: currentTree.stdout,
    releasedTree: releasedTree.stdout,
    errors,
  };
}

function main() {
  const ref = process.argv[2] ?? "HEAD";
  const result = verifyAppEngineRelease(ROOT, ref);
  if (result.errors.length > 0) {
    for (const error of result.errors) console.error(`::error::${error}`);
    process.exit(1);
  }
  console.log(
    `ok  ${ENGINE_TREE} at ${ref} matches annotated ${result.tag} (${result.tagCommit})`,
  );
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
