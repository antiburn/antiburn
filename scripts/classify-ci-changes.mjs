#!/usr/bin/env node
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { appendFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { isDeepStrictEqual } from "node:util";

const APP_RELEASE_FILES = [
  "CHANGELOG.md",
  "apps/desktop/package.json",
  "apps/desktop/src-tauri/Cargo.lock",
  "apps/desktop/src-tauri/Cargo.toml",
  "apps/desktop/src-tauri/tauri.conf.json",
];

const ENGINE_RELEASE_FILES = [
  "apps/desktop/src-tauri/Cargo.lock",
  "crates/antiburn-local/CHANGELOG.md",
  "crates/antiburn-local/Cargo.lock",
  "crates/antiburn-local/Cargo.toml",
];

function sameFiles(actual, expected) {
  return (
    actual.length === expected.length &&
    [...actual]
      .sort()
      .every((file, index) => file === [...expected].sort()[index])
  );
}

function jsonWithVersionPlaceholder(contents) {
  const value = JSON.parse(contents);
  const version = value.version;
  if (typeof version !== "string") return null;
  value.version = "<version>";
  return { version, value };
}

function cargoTomlWithVersionPlaceholder(contents) {
  let inPackage = false;
  let version = null;
  const lines = contents.split(/(\r?\n)/);
  for (let index = 0; index < lines.length; index += 2) {
    const line = lines[index];
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) {
      inPackage = trimmed === "[package]";
      continue;
    }
    if (!inPackage) continue;
    const match = /^(\s*version\s*=\s*")([^"]+)(".*)$/.exec(line);
    if (!match) continue;
    version = match[2];
    lines[index] = `${match[1]}<version>${match[3]}`;
    break;
  }
  return version === null ? null : { version, value: lines.join("") };
}

function cargoLockWithVersionPlaceholder(contents, packageName) {
  let version = null;
  const blocks = contents.split("[[package]]");
  for (let index = 1; index < blocks.length; index += 1) {
    const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(blocks[index]);
    if (!name || name[1] !== packageName) continue;
    blocks[index] = blocks[index].replace(
      /^(\s*version\s*=\s*")([^"]+)(".*)$/m,
      (_whole, prefix, found, suffix) => {
        version = found;
        return `${prefix}<version>${suffix}`;
      },
    );
    break;
  }
  return version === null
    ? null
    : { version, value: blocks.join("[[package]]") };
}

function changedOnlyVersion(before, after, normalizer) {
  const oldValue = normalizer(before);
  const newValue = normalizer(after);
  return (
    oldValue !== null &&
    newValue !== null &&
    oldValue.version !== newValue.version &&
    isDeepStrictEqual(oldValue.value, newValue.value)
  );
}

export function isPureAppReleaseChange(files, readAtRef) {
  if (!sameFiles(files, APP_RELEASE_FILES)) return false;
  return (
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/package.json"),
      readAtRef("head", "apps/desktop/package.json"),
      jsonWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/tauri.conf.json"),
      readAtRef("head", "apps/desktop/src-tauri/tauri.conf.json"),
      jsonWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/Cargo.toml"),
      readAtRef("head", "apps/desktop/src-tauri/Cargo.toml"),
      cargoTomlWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/Cargo.lock"),
      readAtRef("head", "apps/desktop/src-tauri/Cargo.lock"),
      (contents) => cargoLockWithVersionPlaceholder(contents, "antiburn"),
    )
  );
}

export function isPureEngineReleaseChange(files, readAtRef) {
  if (!sameFiles(files, ENGINE_RELEASE_FILES)) return false;
  return (
    changedOnlyVersion(
      readAtRef("base", "crates/antiburn-local/Cargo.toml"),
      readAtRef("head", "crates/antiburn-local/Cargo.toml"),
      cargoTomlWithVersionPlaceholder,
    ) &&
    changedOnlyVersion(
      readAtRef("base", "crates/antiburn-local/Cargo.lock"),
      readAtRef("head", "crates/antiburn-local/Cargo.lock"),
      (contents) => cargoLockWithVersionPlaceholder(contents, "antiburn-local"),
    ) &&
    changedOnlyVersion(
      readAtRef("base", "apps/desktop/src-tauri/Cargo.lock"),
      readAtRef("head", "apps/desktop/src-tauri/Cargo.lock"),
      (contents) => cargoLockWithVersionPlaceholder(contents, "antiburn-local"),
    )
  );
}

function isDocumentation(file) {
  return (
    file.startsWith("docs/") ||
    [
      "CHANGELOG.md",
      "CONTRIBUTING.md",
      "LICENSE",
      "NOTICE",
      "README.md",
      "SECURITY.md",
    ].includes(file)
  );
}

function isDependencyConfiguration(file) {
  return (
    ["package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml"].includes(file) ||
    file === "apps/desktop/package.json" ||
    file.endsWith("/Cargo.toml") ||
    file.endsWith("/Cargo.lock") ||
    file.endsWith("/deny.toml")
  );
}

export function classifyPaths(files) {
  const result = {
    docs_only: files.length > 0 && files.every(isDocumentation),
    frontend: false,
    engine: false,
    desktop_backend: false,
    full: false,
    release_app: false,
    release_engine: false,
  };
  if (result.docs_only) return result;

  for (const file of files) {
    if (isDocumentation(file)) continue;
    if (
      file.startsWith(".github/") ||
      file.startsWith("scripts/") ||
      isDependencyConfiguration(file)
    ) {
      result.full = true;
    } else if (file.startsWith("crates/antiburn-local/")) {
      result.engine = true;
      result.desktop_backend = true;
    } else if (file.startsWith("apps/desktop/src-tauri/")) {
      result.desktop_backend = true;
    } else if (file.startsWith("apps/desktop/")) {
      result.frontend = true;
    } else {
      result.full = true;
    }
  }

  if (result.full) {
    result.frontend = true;
    result.engine = true;
    result.desktop_backend = true;
  }
  return result;
}

export function classifyChanges(files, readAtRef) {
  if (isPureAppReleaseChange(files, readAtRef)) {
    return { ...classifyPaths([]), release_app: true };
  }
  if (isPureEngineReleaseChange(files, readAtRef)) {
    return { ...classifyPaths([]), release_engine: true };
  }
  return classifyPaths(files);
}

function git(...args) {
  return execFileSync("git", args, {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
}

export function listChangedFiles(base, head, runGit = git) {
  // Do not filter statuses here. A deletion can remove executable code or a
  // dependency boundary just as surely as an addition can introduce one.
  return runGit("diff", "--name-only", base, head)
    .split(/\r?\n/)
    .filter(Boolean);
}

function main() {
  const [base, head] = process.argv.slice(2);
  if (!base || !head) {
    console.error("usage: classify-ci-changes.mjs <base-sha> <head-sha>");
    process.exit(2);
  }

  let files;
  try {
    files = listChangedFiles(base, head);
  } catch (error) {
    console.error(
      `::error::Could not classify ${base}..${head}: ${error.message}`,
    );
    process.exit(1);
  }

  const readAtRef = (ref, file) =>
    git("show", `${ref === "base" ? base : head}:${file}`);
  let result;
  try {
    result = classifyChanges(files, readAtRef);
  } catch (error) {
    console.warn(
      `::warning::Semantic release classification failed: ${error.message}`,
    );
    result = {
      ...classifyPaths(files),
      full: true,
      frontend: true,
      engine: true,
      desktop_backend: true,
    };
  }

  console.log(
    `Changed files (${files.length}):\n${files.map((file) => `  ${file}`).join("\n")}`,
  );
  console.log(`Classification: ${JSON.stringify(result)}`);
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      `${Object.entries(result)
        .map(([key, value]) => `${key}=${value}`)
        .join("\n")}\n`,
    );
  }
}

if (
  process.argv[1] &&
  import.meta.url === new URL(`file://${process.argv[1]}`).href
) {
  main();
}
