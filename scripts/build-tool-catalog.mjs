#!/usr/bin/env node
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Tool-catalog compaction.
//
// antiburn/systemprompts holds one `metadata.yml` per released version of
// Claude Code and Codex, and each file lists every measured tool for every
// model that version's system prompt was captured against. That checkout is
// large and not part of this repository. This script reads it once and
// writes a small JSON file the Rust engine embeds at compile time, so the
// running application never needs the checkout, or a network call, to know
// a tool's canonical name, its aliases, and its measured token cost.
//
// The engine's build script (crates/antiburn-local/build.rs) embeds the file
// that the ANTIBURN_TOOL_CATALOG environment variable names. The release
// workflow runs this script and sets that variable. When it is not set, the
// engine embeds the small committed fixture catalogue, so a dev build or a
// CI run needs neither the checkout nor this script.
//
// Usage:
//   node scripts/build-tool-catalog.mjs <systemprompts-checkout> [--out <path>]
//     [--versions claude=<v>,<v>;codex=<v>,<v>]
//
// `--out` defaults to crates/antiburn-local/target/tool_catalog.json.
// `--versions` restricts which version directories are read, so the same
// script cuts the small fixture catalogue from the real checkout. See
// crates/antiburn-local/tests/fixtures/README.md for the exact command used.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { parse as parseYaml } from 'yaml';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_OUT = join(REPO_ROOT, 'crates/antiburn-local/target/tool_catalog.json');

// antiburn/systemprompts' two agent directories, and the catalogue key each
// one compacts to.
const AGENTS = [
  { dirName: 'claude-code', key: 'claude' },
  { dirName: 'codex', key: 'codex' },
];

const VERSION_DIR = /^\d+\.\d+\.\d+$/;

function usage(message) {
  if (message) console.error(`error: ${message}`);
  console.error(
    'usage: build-tool-catalog.mjs <systemprompts-checkout> [--out <path>]' +
      ' [--versions claude=<v>,<v>;codex=<v>,<v>]',
  );
  process.exit(2);
}

export function parseArgs(argv) {
  const positional = [];
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    // `pnpm run tool-catalog -- <checkout>` passes the `--` through to node.
    if (arg === '--') continue;
    if (arg === '--out' || arg === '--versions') {
      const value = argv[index + 1];
      if (value === undefined) usage(`${arg} needs a value`);
      options[arg.slice(2)] = value;
      index += 1;
    } else if (arg.startsWith('--')) {
      usage(`unknown flag: ${arg}`);
    } else {
      positional.push(arg);
    }
  }
  if (positional.length > 1) usage('too many positional arguments');
  return { checkout: positional[0], out: options.out, versions: options.versions };
}

/**
 * `--versions claude=2.1.220,2.1.232;codex=0.146.1` into
 * `{ claude: Set('2.1.220', '2.1.232'), codex: Set('0.146.1') }`.
 */
function parseVersionFilter(spec) {
  const filter = {};
  for (const segment of spec.split(';')) {
    const trimmed = segment.trim();
    if (!trimmed) continue;
    const split = trimmed.indexOf('=');
    if (split === -1) usage(`bad --versions segment: ${trimmed}`);
    const agent = trimmed.slice(0, split).trim();
    const versions = trimmed
      .slice(split + 1)
      .split(',')
      .map((version) => version.trim())
      .filter(Boolean);
    filter[agent] = new Set(versions);
  }
  return filter;
}

/** Ascending order, comparing each dot-separated part as a number. */
export function compareVersions(left, right) {
  const leftParts = left.split('.').map(Number);
  const rightParts = right.split('.').map(Number);
  const length = Math.max(leftParts.length, rightParts.length);
  for (let index = 0; index < length; index += 1) {
    const diff = (leftParts[index] ?? 0) - (rightParts[index] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** Version directory names under an agent directory, each holding a `metadata.yml` file. */
function listVersionDirs(agentDir) {
  if (!existsSync(agentDir)) return [];
  return readdirSync(agentDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && VERSION_DIR.test(entry.name))
    .map((entry) => entry.name)
    .filter((version) => existsSync(join(agentDir, version, 'metadata.yml')));
}

/**
 * The tool surface one version's `metadata.yml` measured, or `null` when the
 * version has no `system_prompts` entry with a measured tool list.
 *
 * Some entries carry a real model name but no `tools` array at all: the
 * capture measured the prompt but not its tool definitions (an unavailable
 * token count, or a model the provider stopped serving). An entry for the
 * `"unrecorded"` model — a captured prompt whose model was never logged — is
 * one instance of this, not a special case. Every such entry is skipped.
 */
function surfaceForVersion(metadataPath) {
  const doc = parseYaml(readFileSync(metadataPath, 'utf8'));
  const tools = new Map();
  for (const entry of doc.system_prompts ?? []) {
    if (!Array.isArray(entry.tools)) continue;
    const model = entry.model;
    for (const tool of entry.tools) {
      let record = tools.get(tool.canonical_name);
      if (!record) {
        record = { aliases: new Set(), tokens: new Map() };
        tools.set(tool.canonical_name, record);
      }
      for (const alias of tool.observed_raw_aliases ?? []) record.aliases.add(alias);
      if (typeof tool.definition_token_count === 'number') {
        record.tokens.set(model, tool.definition_token_count);
      }
    }
  }
  if (tools.size === 0) return null;
  const sortedTools = [...tools.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, record]) => ({
      name,
      aliases: [...record.aliases].sort((left, right) => left.localeCompare(right)),
      tokens: Object.fromEntries(
        [...record.tokens.entries()].sort(([left], [right]) => left.localeCompare(right)),
      ),
    }));
  return { tools: sortedTools };
}

/**
 * One agent's compacted catalogue: every measured version mapped to a
 * surface index, and the deduplicated surfaces those indices point at.
 *
 * Surfaces are deduplicated in first-seen order walking versions oldest to
 * newest, so two versions with an identical tool list, alias set, and token
 * map share one entry instead of repeating it.
 */
export function buildAgentCatalog(agentDir, allowedVersions) {
  let versions = listVersionDirs(agentDir).sort(compareVersions);
  if (allowedVersions) versions = versions.filter((version) => allowedVersions.has(version));

  const surfaces = [];
  const surfaceKeys = [];
  const versionIndex = {};
  for (const version of versions) {
    const surface = surfaceForVersion(join(agentDir, version, 'metadata.yml'));
    if (!surface) continue;
    const key = JSON.stringify(surface);
    let index = surfaceKeys.indexOf(key);
    if (index === -1) {
      surfaces.push(surface);
      surfaceKeys.push(key);
      index = surfaces.length - 1;
    }
    versionIndex[version] = index;
  }
  return { versions: versionIndex, surfaces };
}

function resolveCommit(checkoutPath) {
  try {
    return execFileSync('git', ['-C', checkoutPath, 'rev-parse', 'HEAD'], {
      encoding: 'utf8',
    }).trim();
  } catch (error) {
    usage(`could not resolve a commit for ${checkoutPath}: ${error.message}`);
  }
}

function summarize(catalog) {
  const perAgent = Object.entries(catalog.agents).map(
    ([agent, data]) =>
      `${agent} ${Object.keys(data.versions).length} versions / ${data.surfaces.length} surfaces`,
  );
  return `tool_catalog.json: ${perAgent.join(', ')} (commit ${catalog.source.commit})`;
}

function main() {
  const { checkout, out, versions } = parseArgs(process.argv.slice(2));
  if (!checkout) {
    console.error('error: a systemprompts checkout path is required.');
    process.exit(2);
  }
  const outPath = out ? resolve(out) : DEFAULT_OUT;
  mkdirSync(dirname(outPath), { recursive: true });

  const versionFilter = versions ? parseVersionFilter(versions) : null;
  const commit = resolveCommit(checkout);
  const catalog = {
    schemaVersion: 1,
    source: { repository: 'antiburn/systemprompts', commit },
    agents: Object.fromEntries(
      AGENTS.map(({ dirName, key }) => [
        key,
        buildAgentCatalog(join(checkout, dirName), versionFilter?.[key] ?? null),
      ]),
    ),
  };

  writeFileSync(outPath, `${JSON.stringify(catalog, null, 2)}\n`);
  console.log(summarize(catalog));
}

// --- CLI --------------------------------------------------------------------

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main();
}
