#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const NOTICES_PATH = resolve(ROOT, "THIRD_PARTY_NOTICES");
const GENERATED_START = "----- BEGIN GENERATED DEPENDENCY NOTICES -----";
const GENERATED_END = "----- END GENERATED DEPENDENCY NOTICES -----";
const LEGAL_FILE = /^(licen[cs]e|copying|notice|copyright)(?:[._-].*)?$/i;
const FRONTEND_LICENSE_OVERRIDES = new Map([
  [
    "react-remove-scroll-bar@2.3.8",
    resolve(
      ROOT,
      "scripts/third-party-license-overrides/react-remove-scroll-bar-2.3.8.txt",
    ),
  ],
]);

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

export function frontendPackages(
  inventory,
  resolveLegalTexts = frontendLegalTexts,
) {
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
        const key = `${entry.name}@${version}`;
        packages.push({
          name: entry.name,
          version,
          license: entry.license,
          source:
            entry.homepage ??
            `https://www.npmjs.com/package/${entry.name}/v/${version}`,
          legalTexts: resolveLegalTexts(key, entry.paths ?? []),
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

function normalizeLegalText(text) {
  return `${text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .trim()}\n`;
}

function collectLegalTexts(directory, depth = 0) {
  const texts = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isFile() && LEGAL_FILE.test(entry.name)) {
      texts.push(normalizeLegalText(readFileSync(path, "utf8")));
    } else if (
      entry.isDirectory() &&
      depth < 2 &&
      entry.name !== "node_modules" &&
      !entry.name.startsWith(".")
    ) {
      texts.push(...collectLegalTexts(path, depth + 1));
    }
  }
  return texts;
}

export function frontendLegalTexts(key, paths) {
  const override = FRONTEND_LICENSE_OVERRIDES.get(key);
  const texts = override
    ? [normalizeLegalText(readFileSync(override, "utf8"))]
    : paths.flatMap((path) => collectLegalTexts(path));
  const unique = [...new Set(texts)].sort(compareText);
  if (unique.length === 0) {
    throw new Error(`${key} has no distributable frontend license text`);
  }
  return unique;
}

export function rustPackages(report) {
  return report.crates
    .filter((item) => item.package.source)
    .map((item) => {
      const { package: crate } = item;
      return {
        name: crate.name,
        version: crate.version,
        license: item.license,
        source:
          crate.repository ??
          crate.homepage ??
          `https://crates.io/crates/${crate.name}/${crate.version}`,
      };
    })
    .sort((left, right) =>
      compareText(
        `${left.name}@${left.version}`,
        `${right.name}@${right.version}`,
      ),
    );
}

export function legalBodies(frontend, rustReport) {
  const packagesByText = new Map();
  const add = (text, packageName) => {
    const normalized = normalizeLegalText(text);
    const packages = packagesByText.get(normalized) ?? new Set();
    packages.add(packageName);
    packagesByText.set(normalized, packages);
  };

  for (const item of frontend) {
    for (const text of item.legalTexts)
      add(text, `${item.name}@${item.version}`);
  }
  for (const license of rustReport.licenses) {
    for (const item of license.used_by) {
      if (item.crate.source) {
        add(license.text, `${item.crate.name}@${item.crate.version}`);
      }
    }
  }

  return [...packagesByText]
    .map(([text, packages]) => ({
      text,
      packages: [...packages].sort(compareText),
    }))
    .sort((left, right) =>
      compareText(left.packages.join("\n"), right.packages.join("\n")),
    );
}

export function generatedSection(frontend, rust, bodies) {
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
    "",
    "License and notice texts",
    "------------------------",
    ...bodies.flatMap(({ packages, text }) => [
      "",
      "Used by:",
      ...packages.map((item) => `  ${item}`),
      "",
      text.trimEnd(),
      "",
      "========================================",
    ]),
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
  const cargoAbout = JSON.parse(
    run(
      "cargo",
      [
        "about",
        "generate",
        "--frozen",
        "--fail",
        "--format",
        "json",
        "--manifest-path",
        "Cargo.toml",
      ],
      resolve(ROOT, "apps/desktop/src-tauri"),
    ),
  );
  const current = readFileSync(NOTICES_PATH, "utf8");
  const frontend = frontendPackages(frontendInventory);
  const expected = replaceGeneratedSection(
    current,
    generatedSection(
      frontend,
      rustPackages(cargoAbout),
      legalBodies(frontend, cargoAbout),
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
