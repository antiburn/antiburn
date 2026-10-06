#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function versionFromTag(tag) {
  const match = /^antiburn-v(\d+\.\d+\.\d+)$/.exec(tag);
  if (!match) throw new Error("Expected a stable desktop tag: antiburn-vX.Y.Z");
  return match[1];
}

function singleMatch(contents, pattern, label) {
  const matches = [...contents.matchAll(pattern)];
  if (matches.length !== 1) {
    throw new Error(`Expected exactly one ${label}`);
  }
  return matches[0];
}

export function caskVersion(contents) {
  return singleMatch(
    contents,
    /^  version "(\d+\.\d+\.\d+)"$/gm,
    "cask version",
  )[1];
}

function checksum(contents, filename) {
  const entries = contents.split(/\r?\n/).flatMap((line) => {
    const match = /^(\S+)\s+\*?(\S+)$/.exec(line);
    return match?.[2] === filename ? [match[1]] : [];
  });
  if (entries.length !== 1 || !/^[a-f\d]{64}$/i.test(entries[0])) {
    throw new Error(`Expected exactly one SHA-256 checksum for ${filename}`);
  }
  return entries[0].toLowerCase();
}

function compareVersions(left, right) {
  const a = left.split(".").map(BigInt);
  const b = right.split(".").map(BigInt);
  for (let index = 0; index < 3; index++) {
    if (a[index] !== b[index]) return a[index] > b[index] ? 1 : -1;
  }
  return 0;
}

export function updateCask(contents, tag, release, checksums) {
  const version = versionFromTag(tag);
  if (
    release.tagName !== tag ||
    release.isDraft !== false ||
    release.isPrerelease !== false
  ) {
    throw new Error(
      "The selected release must be published, stable, and match the requested tag",
    );
  }
  const armName = `antiburn_${version}_aarch64.dmg`;
  const intelName = `antiburn_${version}_x64.dmg`;
  for (const name of [armName, intelName, "SHA256SUMS"]) {
    if (release.assets.filter((asset) => asset.name === name).length !== 1) {
      throw new Error(`Expected exactly one release asset: ${name}`);
    }
  }
  const armHash = checksum(checksums, armName);
  const intelHash = checksum(checksums, intelName);
  const current = caskVersion(contents);
  singleMatch(contents, /^  sha256\b.*$/gm, "cask checksum declaration");
  const hashes = singleMatch(
    contents,
    /^  sha256 arm:( +)"([a-f\d]{64})",\n( +)intel:( +)"([a-f\d]{64})"$/gm,
    "architecture checksum stanza",
  );
  const order = compareVersions(version, current);
  if (order < 0)
    throw new Error(
      `Refusing to downgrade the cask from ${current} to ${version}`,
    );
  if (order === 0) {
    if (armHash !== hashes[2] || intelHash !== hashes[5]) {
      throw new Error(
        `Release checksums changed for existing version ${version}`,
      );
    }
    return contents;
  }
  return contents
    .replace(/^  version "\d+\.\d+\.\d+"$/m, `  version "${version}"`)
    .replace(
      hashes[0],
      `  sha256 arm:${hashes[1]}"${armHash}",\n${hashes[3]}intel:${hashes[4]}"${intelHash}"`,
    );
}

function gh(...args) {
  return execFileSync("gh", args, {
    encoding: "utf8",
    timeout: 60_000,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function main() {
  const [tag, destination, ...extra] = process.argv.slice(2);
  if (!tag || !destination || extra.length) {
    throw new Error(
      "Usage: node scripts/update-homebrew-cask.mjs <tag> <cask-path>",
    );
  }
  versionFromTag(tag);
  const release = JSON.parse(
    gh(
      "release",
      "view",
      tag,
      "--repo",
      "antiburn/antiburn",
      "--json",
      "tagName,isDraft,isPrerelease,assets",
    ),
  );
  if (
    release.isDraft !== false ||
    release.isPrerelease !== false ||
    release.tagName !== tag
  ) {
    throw new Error(
      "The selected release must be published, stable, and match the requested tag",
    );
  }
  const checksums = gh(
    "release",
    "download",
    tag,
    "--repo",
    "antiburn/antiburn",
    "--pattern",
    "SHA256SUMS",
    "--output",
    "-",
  );
  const before = readFileSync(destination, "utf8");
  const after = updateCask(before, tag, release, checksums);
  if (after === before) {
    console.log(`Cask already matches ${tag}`);
  } else {
    writeFileSync(destination, after);
    console.log(`Updated cask to ${tag}`);
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
