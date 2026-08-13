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
// - NOTICE      — the copyright holder's legal name
// - crates/antiburn-local/tests/boundary.rs — the engine's pattern table
// - this script — its own pattern table
//
// Conformance audits that must name prohibited concepts are governance
// records and live in the private repository, not here — so this scan
// covers the complete public tree without directory-level exemptions.

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

/** Whether a repository-relative path is exempt from the concept scan. */
function isExempt(rel) {
  return EXEMPT.has(rel.split(path.sep).join('/'));
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
