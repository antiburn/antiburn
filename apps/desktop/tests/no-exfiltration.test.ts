// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The no-exfiltration guarantee, enforced mechanically.
 *
 * antiburn is local in one exact sense: it needs no connection to any
 * service of ours. The renderer may call a provider's API with the
 * reader's own credentials, and `fetch`/`XMLHttpRequest`/`WebSocket` are
 * ordinary, permitted capabilities. The one hard line is
 * disclosure: the renderer reaches no service of ours. Three layers hold
 * that up:
 *
 * 1. The engine's own `tests/boundary.rs` keeps `antiburn-local` free of
 *    telemetry/analytics SDKs and of proprietary/commercial provenance.
 * 2. `scripts/check-boundary.mjs` scans the whole repository for prohibited
 *    concepts, including telemetry SDKs and commercial identities.
 * 3. **This test** covers the remaining surface: the renderer, where an
 *    imported analytics SDK or a hardcoded reporting host would be invisible
 *    to both of the above.
 *
 * There are two deliberate exceptions, and both live in the shell rather than
 * here. The updater plugin is registered in release builds only and talks to
 * antiburn's own release feed — a feed that carries no reader data. The
 * anonymised usage-analytics channel (`src-tauri/src/usage_analytics`, recorded
 * as D-026 and deviations D-28) reports on the application itself, behind a
 * control the reader meets before anything is sent. Neither is reachable from
 * the renderer: both are Rust-side and invoked over IPC, so they need no browser
 * networking API and do not appear here. That is the point of this file
 * continuing to pass unchanged — the renderer, which is the half that handles
 * session content, still reaches nothing.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

/**
 * The renderer's source tree. This guard deliberately lives *outside* it: the
 * pattern table below names every banned SDK and host, and a checker that
 * trips its own check is a checker nobody can grep past.
 */
const SOURCE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src');

/** Extensions worth reading. Everything else in `src/` is styles or assets. */
const CODE_EXTENSIONS = new Set(['.ts', '.tsx']);

/**
 * Telemetry, analytics, and crash-reporting SDKs. Each is a distinct way for
 * the renderer to phone home, so the list is the *capability* surface rather
 * than a set of spellings of one call.
 */
const TELEMETRY_IMPORTS = [
  { pattern: /['"]@sentry\//, name: '@sentry/*' },
  { pattern: /['"]posthog-js/, name: 'posthog-js' },
  { pattern: /['"]@segment\//, name: '@segment/*' },
  { pattern: /['"]analytics-node/, name: 'analytics-node' },
  { pattern: /['"]@amplitude\//, name: '@amplitude/*' },
  { pattern: /['"]amplitude-js/, name: 'amplitude-js' },
  { pattern: /['"]mixpanel-browser/, name: 'mixpanel-browser' },
  { pattern: /['"]react-ga4?['"]/, name: 'react-ga' },
  { pattern: /['"]@datadog\//, name: '@datadog/*' },
  { pattern: /['"]@fullstory\//, name: '@fullstory/*' },
  { pattern: /['"]logrocket/i, name: 'LogRocket' },
];

/**
 * Hostnames operated by antiburn or by a telemetry/analytics vendor. A
 * hardcoded occurrence anywhere in the renderer means data destined to
 * leave the process to a party that does not already hold it.
 */
const TELEMETRY_HOSTS = [
  'sentry.io',
  'ingest.sentry.io',
  'posthog.com',
  'i.posthog.com',
  'segment.io',
  'api.segment.io',
  'google-analytics.com',
  'googletagmanager.com',
  'mixpanel.com',
  'amplitude.com',
  'datadoghq.com',
  'fullstory.com',
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

describe('the renderer reaches no service of ours', () => {
  it('imports no telemetry, analytics, or crash-reporting SDK anywhere in src/', () => {
    const violations: string[] = [];

    for (const file of sourceFiles(SOURCE_ROOT)) {
      const relative = path.relative(SOURCE_ROOT, file);
      const contents = readFileSync(file, 'utf8');
      for (const { pattern, name } of TELEMETRY_IMPORTS) {
        if (pattern.test(contents)) violations.push(`${relative}: imports ${name}`);
      }
      for (const host of TELEMETRY_HOSTS) {
        if (contents.includes(host)) violations.push(`${relative}: references ${host}`);
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
