import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function validateAppVersion(version) {
  const separator = version.indexOf("-");
  const base = separator === -1 ? version : version.slice(0, separator);
  const prerelease = separator === -1 ? null : version.slice(separator + 1);
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(base)) {
    throw new Error(`Unsupported application version: ${version}`);
  }
  if (
    prerelease !== null &&
    !prerelease
      .split(".")
      .every(
        (part) =>
          /^[0-9A-Za-z-]+$/.test(part) &&
          (!/^\d+$/.test(part) || /^(0|[1-9]\d*)$/.test(part)),
      )
  ) {
    throw new Error(`Unsupported prerelease version: ${version}`);
  }
}

function replaceVersionField(contents, version, label) {
  const matches = [...contents.matchAll(/^version\s*=\s*"[^"]+"/gm)];
  if (matches.length !== 1) {
    throw new Error(`Expected one package version in ${label}`);
  }
  return contents.replace(/^version\s*=\s*"[^"]+"/m, `version = "${version}"`);
}

function updateManifest(contents, version, label) {
  const section = /^\[package\]\r?\n[\s\S]*?(?=^\[|$(?![\s\S]))/m.exec(
    contents,
  );
  if (!section) throw new Error(`Missing package section in ${label}`);
  const updated = replaceVersionField(section[0], version, label);
  return (
    contents.slice(0, section.index) +
    updated +
    contents.slice(section.index + section[0].length)
  );
}

function updateLockfile(contents, packages, version, label) {
  const found = new Set();
  const updated = contents.replace(
    /\[\[package\]\][\s\S]*?(?=\[\[package\]\]|$)/g,
    (block) => {
      const name = /^name\s*=\s*"([^"]+)"/m.exec(block)?.[1];
      if (!packages.includes(name)) return block;
      if (found.has(name))
        throw new Error(`Duplicate ${name} package in ${label}`);
      found.add(name);
      return replaceVersionField(block, version, `${label}: ${name}`);
    },
  );
  if (found.size !== packages.length)
    throw new Error(`Missing application package in ${label}`);
  return updated;
}

export function prepareAppVersion(root, version) {
  validateAppVersion(version);
  const files = [
    ["apps/desktop/package.json", "json"],
    ["apps/desktop/src-tauri/tauri.conf.json", "json"],
    ["apps/desktop/src-tauri/Cargo.toml", "toml"],
    ["apps/desktop/src-tauri/Cargo.lock", ["antiburn", "antiburn-remote"]],
    ["crates/antiburn-remote/Cargo.toml", "toml"],
    ["crates/antiburn-remote/Cargo.lock", ["antiburn-remote"]],
  ];
  const changes = files.map(([file, type]) => {
    const contents = readFileSync(join(root, file), "utf8");
    let updated;
    if (type === "json") {
      const manifest = JSON.parse(contents);
      if (typeof manifest.version !== "string")
        throw new Error(`Missing version in ${file}`);
      manifest.version = version;
      updated = `${JSON.stringify(manifest, null, 2)}\n`;
    } else if (type === "toml") {
      updated = updateManifest(contents, version, file);
    } else {
      updated = updateLockfile(contents, type, version, file);
    }
    return [file, updated];
  });
  for (const [file, contents] of changes)
    writeFileSync(join(root, file), contents);
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  prepareAppVersion(
    resolve(dirname(fileURLToPath(import.meta.url)), ".."),
    process.argv[2] ?? "",
  );
}
