#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const NOTICES_PATH = resolve(ROOT, "THIRD_PARTY_NOTICES");
const GENERATED_START = "----- BEGIN GENERATED DEPENDENCY NOTICES -----";
const GENERATED_END = "----- END GENERATED DEPENDENCY NOTICES -----";

const ALLOWED_FRONTEND_LICENSES = new Set([
  "0BSD",
  "Apache-2.0 OR MIT",
  "BSD-3-Clause",
  "CC0-1.0",
  "ISC",
  "MIT",
  "MIT AND ISC",
  "MIT OR Apache-2.0",
  "OFL-1.1",
]);

const ALLOWED_RUST_LICENSES = new Set([
  "(Apache-2.0 OR MIT) AND BSD-3-Clause",
  "(MIT OR Apache-2.0) AND Unicode-3.0",
  "0BSD",
  "0BSD OR MIT OR Apache-2.0",
  "Apache-2.0",
  "Apache-2.0 / MIT",
  "Apache-2.0 AND ISC",
  "Apache-2.0 AND MIT",
  "Apache-2.0 OR BSL-1.0",
  "Apache-2.0 OR ISC OR MIT",
  "Apache-2.0 OR MIT",
  "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
  "Apache-2.0 WITH LLVM-exception",
  "Apache-2.0/MIT",
  "BSD-2-Clause",
  "BSD-2-Clause OR Apache-2.0 OR MIT",
  "BSD-2-Clause OR MIT OR Apache-2.0",
  "BSD-3-Clause",
  "BSD-3-Clause AND MIT",
  "BSD-3-Clause OR Apache-2.0",
  "BSD-3-Clause OR MIT OR Apache-2.0",
  "BSD-3-Clause/MIT",
  "CC0-1.0",
  "CC0-1.0 OR MIT-0 OR Apache-2.0",
  "CDLA-Permissive-2.0",
  "ISC",
  "MIT",
  "MIT OR Apache-2.0",
  "MIT OR Apache-2.0 OR LGPL-2.1-or-later",
  "MIT OR Apache-2.0 OR Zlib",
  "MIT OR Zlib OR Apache-2.0",
  "MIT-0",
  "MIT/Apache-2.0",
  "MPL-2.0",
  "Unicode-3.0",
  "Unlicense",
  "Unlicense OR MIT",
  "Unlicense/MIT",
  "Zlib",
  "Zlib OR Apache-2.0 OR MIT",
]);

const RUST_LICENSE_OVERRIDES = new Map([
  [
    "tauri-nspanel@2.1.0|git+https://github.com/ahkohd/tauri-nspanel?rev=a3122e894383aa068ec5365a42994e3ac94ba1b6#a3122e894383aa068ec5365a42994e3ac94ba1b6",
    {
      license: "MIT OR Apache-2.0",
      source:
        "https://github.com/ahkohd/tauri-nspanel/tree/a3122e894383aa068ec5365a42994e3ac94ba1b6",
    },
  ],
]);

function compareText(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function run(command, args, cwd) {
  return execFileSync(command, args, {
    cwd,
    encoding: "utf8",
    maxBuffer: 128 * 1024 * 1024,
  });
}

export function frontendPackages(inventory) {
  const packages = [];
  for (const [groupLicense, entries] of Object.entries(inventory)) {
    for (const entry of entries) {
      if (entry.license !== groupLicense) {
        throw new Error(
          `${entry.name} has inconsistent frontend license metadata`,
        );
      }
      if (!ALLOWED_FRONTEND_LICENSES.has(entry.license)) {
        throw new Error(
          `unreviewed frontend license ${entry.license} on ${entry.name}`,
        );
      }
      for (const version of entry.versions) {
        packages.push({
          name: entry.name,
          version,
          license: entry.license,
          source:
            entry.homepage ??
            `https://www.npmjs.com/package/${entry.name}/v/${version}`,
        });
      }
    }
  }
  return packages.sort((left, right) =>
    compareText(
      `${left.name}@${left.version}`,
      `${right.name}@${right.version}`,
    ),
  );
}

export function rustPackages(metadata) {
  const packagesById = new Map(
    metadata.packages.map((item) => [item.id, item]),
  );
  const nodesById = new Map(
    metadata.resolve.nodes.map((node) => [node.id, node]),
  );
  const pending = [...metadata.workspace_members];
  const visited = new Set();

  while (pending.length > 0) {
    const id = pending.pop();
    if (visited.has(id)) continue;
    visited.add(id);
    const node = nodesById.get(id);
    if (!node)
      throw new Error(`Cargo metadata has no dependency node for ${id}`);
    for (const dependency of node.deps) {
      if (dependency.dep_kinds.some(({ kind }) => kind === null)) {
        pending.push(dependency.pkg);
      }
    }
  }

  return [...visited]
    .map((id) => packagesById.get(id))
    .filter((item) => item?.source)
    .map((item) => {
      const override = RUST_LICENSE_OVERRIDES.get(
        `${item.name}@${item.version}|${item.source}`,
      );
      const license = item.license ?? override?.license;
      if (!license)
        throw new Error(
          `${item.name} ${item.version} has no Rust license metadata`,
        );
      if (!ALLOWED_RUST_LICENSES.has(license)) {
        throw new Error(
          `unreviewed Rust license ${license} on ${item.name} ${item.version}`,
        );
      }
      return {
        name: item.name,
        version: item.version,
        license,
        source:
          override?.source ??
          item.repository ??
          item.homepage ??
          `https://crates.io/crates/${item.name}/${item.version}`,
      };
    })
    .sort((left, right) =>
      compareText(
        `${left.name}@${left.version}`,
        `${right.name}@${right.version}`,
      ),
    );
}

export function generatedSection(frontend, rust) {
  const lines = [
    GENERATED_START,
    "This section is generated by `pnpm notices`. Do not edit it by hand.",
    "It lists locked production dependencies for the desktop distribution.",
    "",
    "Frontend packages",
    "-----------------",
    ...frontend.map(
      ({ name, version, license, source }) =>
        `${name}@${version} | ${license} | ${source}`,
    ),
    "",
    "Rust crates",
    "-----------",
    ...rust.map(
      ({ name, version, license, source }) =>
        `${name}@${version} | ${license} | ${source}`,
    ),
    GENERATED_END,
  ];
  return `${lines.join("\n")}\n`;
}

export function replaceGeneratedSection(current, generated) {
  const start = current.indexOf(GENERATED_START);
  const end = current.indexOf(GENERATED_END);
  if (start === -1 || end === -1 || end < start) {
    throw new Error(
      "THIRD_PARTY_NOTICES does not contain valid generated-section markers",
    );
  }
  return `${current.slice(0, start)}${generated}${current.slice(end + GENERATED_END.length).replace(/^\n?/, "")}`;
}

function main() {
  const check = process.argv.slice(2).includes("--check");
  const frontendInventory = JSON.parse(
    run(
      "pnpm",
      ["-C", "apps/desktop", "licenses", "list", "--json", "--prod"],
      ROOT,
    ),
  );
  const cargoMetadata = JSON.parse(
    run(
      "cargo",
      ["metadata", "--locked", "--format-version", "1"],
      resolve(ROOT, "apps/desktop/src-tauri"),
    ),
  );
  const current = readFileSync(NOTICES_PATH, "utf8");
  const expected = replaceGeneratedSection(
    current,
    generatedSection(
      frontendPackages(frontendInventory),
      rustPackages(cargoMetadata),
    ),
  );

  if (check) {
    if (current !== expected) {
      console.error("THIRD_PARTY_NOTICES is stale. Run `pnpm notices`.");
      process.exitCode = 1;
    }
    return;
  }
  writeFileSync(NOTICES_PATH, expected);
}

if (
  process.argv[1] &&
  import.meta.url === new URL(`file://${process.argv[1]}`).href
) {
  main();
}
