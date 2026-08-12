// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The offline guarantee, enforced mechanically.
 *
 * antiburn analyses what is already on the machine. Nothing it computes leaves
 * the process, and nothing it renders is fetched. Three layers hold that up:
 *
 * 1. The engine's own `tests/boundary.rs` keeps `antiburn-local` free of
 *    network and socket dependencies.
 * 2. `scripts/check-boundary.mjs` scans the whole repository for prohibited
 *    concepts, including telemetry SDKs and raw socket types.
 * 3. **This test** covers the remaining surface: the renderer, where a single
 *    `fetch()` would be invisible to both of the above.
 *
 * The one deliberate exception is the updater plugin, which is registered in
 * release builds only and talks to a release feed — never to anything carrying
 * session data. It is a Tauri plugin invoked over IPC, so it needs no browser
 * networking API and does not appear here.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

/**
 * The renderer's source tree. This guard deliberately lives *outside* it: the
 * pattern table below names every banned API, and a checker that trips its own
 * check is a checker nobody can grep past.
 */
const SOURCE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src');

/** Extensions worth reading. Everything else in `src/` is styles or assets. */
const CODE_EXTENSIONS = new Set(['.ts', '.tsx']);

/**
 * Browser networking APIs. Each is a distinct way to open a connection, so the
 * list is the *capability* surface rather than a set of spellings of one call.
 */
const NETWORK_APIS = [
  { pattern: /\bfetch\s*\(/, name: 'fetch()' },
  { pattern: /\bXMLHttpRequest\b/, name: 'XMLHttpRequest' },
  { pattern: /\bnew\s+WebSocket\b/, name: 'WebSocket' },
  { pattern: /\bnew\s+EventSource\b/, name: 'EventSource' },
  { pattern: /\bnavigator\.sendBeacon\b/, name: 'navigator.sendBeacon' },
  { pattern: /\bimport\s*\(\s*['"`]https?:/, name: 'a remote dynamic import' },
];

function sourceFiles(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) {
      sourceFiles(full, found);
    } else if (CODE_EXTENSIONS.has(path.extname(entry))) {
      found.push(full);
    }
  }
  return found;
}

describe('the renderer is offline', () => {
  it('opens no network connection anywhere in src/', () => {
    const violations: string[] = [];

    for (const file of sourceFiles(SOURCE_ROOT)) {
      const relative = path.relative(SOURCE_ROOT, file);
      const contents = readFileSync(file, 'utf8');
      for (const { pattern, name } of NETWORK_APIS) {
        if (pattern.test(contents)) violations.push(`${relative}: uses ${name}`);
      }
    }

    expect(violations).toEqual([]);
  });

  it('actually reads the source tree, so a passing run means something', () => {
    // A guard that silently scans nothing is worse than no guard: this pins the
    // walk to a file it must always find.
    const files = sourceFiles(SOURCE_ROOT).map((file) => path.relative(SOURCE_ROOT, file));
    expect(files).toContain(path.join('lib', 'ipc.ts'));
    expect(files.length).toBeGreaterThan(20);
  });
});
