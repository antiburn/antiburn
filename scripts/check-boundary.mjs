#!/usr/bin/env node
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Whole-tree source-boundary check.
//
// The engine's `tests/boundary.rs` enforces the network-free contract for
// `crates/antiburn-local`; this script extends the prohibited-concept scan to
// the ENTIRE repository tree (app, docs, CI), so commercial identities,
// telemetry SDKs, and live-probe concepts cannot land anywhere.
//
// Exemptions (each is a place that legitimately names the concepts):
// - docs/oss/*  — the governance manifests' own rule text
// - docs/audits/** — this repository's own conformance audits (see below)
// - NOTICE      — the copyright holder's legal name
// - crates/antiburn-local/tests/boundary.rs — the engine's pattern table
// - this script — its own pattern table
//
// `docs/audits/**` is exempt as a directory rather than file by file. An audit
// of a boundary has to NAME the concepts it is auditing — the private product,
// the SDKs, the probe utilities — or it degrades into circumlocution that
// nobody can act on, and the repository ends up unable to document its own
// decisions without failing CI. The exemption is prose-only by construction:
// nothing under `docs/audits/` is compiled, linked, or shipped, so a banned
// token there is a word rather than a dependency. Code and configuration
// elsewhere in the tree remain covered without exception.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');

const EXEMPT = new Set([
  'NOTICE',
  'docs/oss/source-allowlist.toml',
  'docs/oss/source-denylist.toml',
  'crates/antiburn-local/tests/boundary.rs',
  'scripts/check-boundary.mjs',
]);

/** Directory prefixes exempt wholesale. Paths are compared with `/`. */
const EXEMPT_PREFIXES = ['docs/audits/'];

/** Whether a repository-relative path is exempt from the concept scan. */
function isExempt(rel) {
  const posix = rel.split(path.sep).join('/');
  return EXEMPT.has(posix) || EXEMPT_PREFIXES.some((prefix) => posix.startsWith(prefix));
}

const SKIP_DIRS = new Set(['.git', 'node_modules', 'target', 'dist']);

const TEXT_EXTENSIONS = new Set([
  '.rs', '.ts', '.tsx', '.js', '.mjs', '.css', '.md', '.json', '.jsonl',
  '.toml', '.yml', '.yaml', '.html', '.txt', '.plist',
]);

// Case-insensitive concept tokens derived from the denylist's concept rules.
const FORBIDDEN_ANY_CASE = [
  'cadence',
  'teamcadence',
  'sentry',
  'litellm',
  'stripe',
  'csrf',
  'lsof',
  'netstat',
  'targetorgid',
  'radar',
  'tune-up',
];

// Case-sensitive code tokens. `127.0.0.1` is allowed ONLY in the two dev-server
// config files (Vite bind address), never in engine or runtime code.
const FORBIDDEN_CODE = ['reqwest::', 'TcpListener', 'UdpSocket'];
const DEV_SERVER_FILES = new Set([
  'apps/desktop/vite.config.ts',
  'apps/desktop/src-tauri/tauri.conf.json',
]);

function walk(dir, files = []) {
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    const rel = path.relative(ROOT, full);
    const stats = statSync(full, { throwIfNoEntry: false });
    if (!stats) continue;
    if (stats.isDirectory()) {
      if (!SKIP_DIRS.has(entry)) walk(full, files);
    } else if (TEXT_EXTENSIONS.has(path.extname(entry)) || entry === 'NOTICE') {
      files.push(rel);
    }
  }
  return files;
}

const violations = [];
for (const rel of walk(ROOT)) {
  if (isExempt(rel)) continue;
  const content = readFileSync(path.join(ROOT, rel), 'utf8');
  const lower = content.toLowerCase();
  for (const token of FORBIDDEN_ANY_CASE) {
    if (lower.includes(token)) violations.push(`${rel}: contains ${JSON.stringify(token)}`);
  }
  for (const token of FORBIDDEN_CODE) {
    if (content.includes(token)) violations.push(`${rel}: contains ${JSON.stringify(token)}`);
  }
  if (content.includes('127.0.0.1') && !DEV_SERVER_FILES.has(rel)) {
    violations.push(`${rel}: contains "127.0.0.1" outside the dev-server configs`);
  }
}

if (violations.length > 0) {
  console.error('source-boundary violations:\n' + violations.join('\n'));
  process.exit(1);
}
console.log('source boundary clean');
