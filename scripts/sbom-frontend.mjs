#!/usr/bin/env node
// CycloneDX 1.6 SBOM for the desktop frontend's production dependency tree.
//
// Why this is a script here rather than an off-the-shelf generator: the
// published CycloneDX tools for npm read an npm-installed tree (an
// `npm ls`/arborist view or a `package-lock.json`), and this workspace is
// resolved by pnpm. The tools that do understand `pnpm-lock.yaml` are large,
// unpinnable-by-digest packages that would have to be fetched and executed
// inside the release pipeline. Trading ~120 reviewable lines for that is a bad
// deal in a repository whose whole premise is that you can read what it runs.
//
// The input is pnpm's own resolved view of what ships:
//
//   pnpm -C apps/desktop licenses list --json --prod > licenses.json
//   node scripts/sbom-frontend.mjs \
//     --input licenses.json --name @antiburn/desktop --version 1.2.3 \
//     --timestamp 2026-01-01T00:00:00Z --output frontend.cdx.json
//
// `--prod` is the point: dev dependencies (the linter, the test runner, the
// bundler) are not in the shipped bundle and do not belong in an SBOM that
// describes it.
//
// Output is deterministic — components sorted, no local paths, timestamp
// supplied by the caller — so two runs over the same lockfile produce the same
// bytes and a reviewer can diff two releases.

import { readFileSync, writeFileSync } from 'node:fs';

function usage(message) {
  if (message) console.error(`error: ${message}`);
  console.error(
    'usage: sbom-frontend.mjs --input <licenses.json> --name <name> --version <version>' +
      ' [--spec-version 1.5|1.6] [--timestamp <iso8601>] [--output <file>]',
  );
  process.exit(2);
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag.startsWith('--') || value === undefined) usage(`bad argument: ${flag}`);
    args[flag.slice(2)] = value;
  }
  return args;
}

/**
 * Percent-encode a package name for a purl, per the package-url spec: the
 * npm namespace (`@scope`) is a separate path segment and its `@` is encoded.
 */
function purlFor(name, version) {
  const [scope, bare] = name.startsWith('@') ? name.split('/') : [null, name];
  const encodedName = encodeURIComponent(bare);
  const namespace = scope ? `${encodeURIComponent(scope)}/` : '';
  return `pkg:npm/${namespace}${encodedName}@${encodeURIComponent(version)}`;
}

/**
 * CycloneDX licence entry.
 *
 * A bare SPDX identifier goes in as an `id`; anything compound (`MIT OR
 * Apache-2.0`, `(MIT AND BSD-3-Clause)`) is an SPDX *expression* and a
 * different field. pnpm reports `Unknown` when a package declares nothing,
 * which is a fact worth carrying rather than dropping.
 */
function licenseEntry(license) {
  if (!license || license === 'Unknown') return [{ license: { name: 'Unknown' } }];
  if (/[()]|\s(?:OR|AND|WITH)\s/.test(license)) return [{ expression: license }];
  return [{ license: { id: license } }];
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.input || !args.name || !args.version) usage('missing required argument');

  // Kept as a flag so both inventories in a release can state the same spec
  // version: the Rust generator tops out below the newest one.
  const specVersion = args['spec-version'] ?? '1.6';
  if (!['1.5', '1.6'].includes(specVersion)) usage(`unsupported spec version: ${specVersion}`);

  const grouped = JSON.parse(readFileSync(args.input, 'utf8'));
  if (grouped === null || typeof grouped !== 'object' || Array.isArray(grouped)) {
    usage('input is not the object `pnpm licenses list --json` produces');
  }

  /** @type {Map<string, object>} keyed by purl so a duplicate cannot be listed twice. */
  const components = new Map();
  for (const [license, packages] of Object.entries(grouped)) {
    for (const pkg of packages) {
      // pnpm groups by licence and collapses a package's versions into a list;
      // each version is its own component.
      for (const version of pkg.versions ?? []) {
        const purl = purlFor(pkg.name, version);
        if (components.has(purl)) continue;
        const component = {
          type: 'library',
          'bom-ref': purl,
          name: pkg.name,
          version,
          purl,
          licenses: licenseEntry(pkg.license ?? license),
        };
        // Never the `paths` pnpm reports: those are absolute paths on whichever
        // machine ran the build, and they describe nothing about the artifact.
        if (pkg.description) component.description = pkg.description;
        if (pkg.homepage) {
          component.externalReferences = [{ type: 'website', url: pkg.homepage }];
        }
        components.set(purl, component);
      }
    }
  }

  const sorted = [...components.values()].sort(
    (left, right) => left.purl.localeCompare(right.purl) || 0,
  );

  const rootPurl = purlFor(args.name, args.version);
  const bom = {
    bomFormat: 'CycloneDX',
    specVersion,
    version: 1,
    metadata: {
      timestamp: args.timestamp ?? new Date().toISOString(),
      tools: {
        components: [
          {
            type: 'application',
            name: 'sbom-frontend.mjs',
            version: '1',
            description: "antiburn's own CycloneDX writer over `pnpm licenses list --json --prod`",
          },
        ],
      },
      component: {
        type: 'application',
        'bom-ref': rootPurl,
        name: args.name,
        version: args.version,
        purl: rootPurl,
        licenses: [{ license: { id: 'MIT' } }],
        description: 'antiburn desktop application frontend bundle (production dependencies)',
      },
    },
    components: sorted,
  };

  const json = `${JSON.stringify(bom, null, 2)}\n`;
  if (args.output) {
    writeFileSync(args.output, json);
    console.log(`wrote ${args.output} (${sorted.length} components)`);
  } else {
    process.stdout.write(json);
  }
}

main();
