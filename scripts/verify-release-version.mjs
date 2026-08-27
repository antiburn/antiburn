#!/usr/bin/env node
// Release tag / manifest agreement check.
//
// A release tag is a claim about the tree it points at, and the tree states its
// own version in several places at once. When those disagree the artifacts lie:
// the installer says one version, the updater manifest another, and the crate a
// third. This script refuses the release instead.
//
// It is the first thing both release workflows run, before any toolchain is
// installed, so the cheapest possible failure catches the most common mistake
// (bumping some of the manifests and tagging anyway).
//
// The lockfiles are checked too, and deliberately: both release builds run with
// `--locked`, so a version bump that did not refresh `Cargo.lock` fails much
// later, on a signing runner, with an error that says nothing about the tag.
//
// Usage:
//   node scripts/verify-release-version.mjs app    antiburn-v1.2.3
//   node scripts/verify-release-version.mjs engine antiburn-local-v0.4.0
//
// On success it writes `version=`, `prerelease=` and `tag=` to $GITHUB_OUTPUT
// when that variable is set, and prints the resolved version on stdout.

import { appendFileSync, readFileSync } from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');

/** What each component tags itself as, and which files must agree. */
const COMPONENTS = {
  app: {
    prefix: 'antiburn-v',
    sources: [
      { file: 'apps/desktop/package.json', read: packageJsonVersion },
      { file: 'apps/desktop/src-tauri/tauri.conf.json', read: packageJsonVersion },
      { file: 'apps/desktop/src-tauri/Cargo.toml', read: cargoTomlVersion },
      { file: 'apps/desktop/src-tauri/Cargo.lock', read: cargoLockVersion('antiburn') },
    ],
  },
  engine: {
    prefix: 'antiburn-local-v',
    sources: [
      { file: 'crates/antiburn-local/Cargo.toml', read: cargoTomlVersion },
      { file: 'crates/antiburn-local/Cargo.lock', read: cargoLockVersion('antiburn-local') },
    ],
  },
};

/** `version` of a JSON manifest (package.json, tauri.conf.json). */
function packageJsonVersion(contents) {
  const parsed = JSON.parse(contents);
  return typeof parsed.version === 'string' ? parsed.version : null;
}

/** `version` from a Cargo.toml `[package]` section, ignoring every other one. */
function cargoTomlVersion(contents) {
  let inPackage = false;
  for (const rawLine of contents.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line.startsWith('[')) {
      inPackage = line === '[package]';
      continue;
    }
    if (!inPackage) continue;
    const match = /^version\s*=\s*"([^"]+)"/.exec(line);
    if (match) return match[1];
  }
  return null;
}

/** `version` of one named package in a Cargo.lock. */
function cargoLockVersion(crateName) {
  return (contents) => {
    const blocks = contents.split('[[package]]');
    for (const block of blocks) {
      const name = /^\s*name\s*=\s*"([^"]+)"/m.exec(block);
      if (!name || name[1] !== crateName) continue;
      const version = /^\s*version\s*=\s*"([^"]+)"/m.exec(block);
      if (version) return version[1];
    }
    return null;
  };
}

/**
 * Semantic versions this project is willing to release.
 *
 * Deliberately narrower than the semver grammar: no build metadata (`+…`),
 * because a `+` is not legal in a Cargo package version requirement's usual
 * shape here and reads as noise in an installer filename, and the pre-release
 * part is restricted to dot-separated alphanumerics.
 */
const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

function fail(message) {
  // The `::error::` prefix is what turns this into an annotation on the run.
  console.error(`::error::${message}`);
  process.exitCode = 1;
}

function main() {
  const [componentName, tag] = process.argv.slice(2);
  const component = COMPONENTS[componentName];
  if (!component || !tag) {
    console.error('usage: verify-release-version.mjs <app|engine> <tag>');
    process.exit(2);
  }

  if (!tag.startsWith(component.prefix)) {
    fail(`${componentName} releases must be tagged ${component.prefix}<version>; got ${tag}`);
    process.exit(1);
  }
  const version = tag.slice(component.prefix.length);
  if (!VERSION_PATTERN.test(version)) {
    fail(`${tag} does not carry a supported version (expected ${component.prefix}X.Y.Z[-pre])`);
    process.exit(1);
  }

  let disagreed = false;
  for (const source of component.sources) {
    const full = path.join(ROOT, source.file);
    let found;
    try {
      found = source.read(readFileSync(full, 'utf8'));
    } catch (error) {
      fail(`${source.file}: could not be read (${error.message})`);
      disagreed = true;
      continue;
    }
    if (found === null) {
      fail(`${source.file}: no version found where one is required`);
      disagreed = true;
    } else if (found !== version) {
      fail(`${source.file}: declares ${found}, but ${tag} claims ${version}`);
      disagreed = true;
    } else {
      console.log(`ok  ${source.file} = ${found}`);
    }
  }

  if (disagreed) {
    console.error(
      'Bump every manifest above (and refresh the lockfile with a build) before tagging.',
    );
    process.exit(1);
  }

  const prerelease = version.includes('-');
  console.log(`version ${version}${prerelease ? ' (pre-release)' : ''}`);
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      `version=${version}\nprerelease=${prerelease}\ntag=${tag}\n`,
    );
  }
}

main();
