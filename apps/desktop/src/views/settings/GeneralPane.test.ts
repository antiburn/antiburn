// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from 'vitest';

import type { ScanStatus } from '../../lib/ipc';
import { byteLabel, scanSummary } from './GeneralPane';

function status(overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    running: false,
    completedAgents: 0,
    totalAgents: 0,
    sessions: 0,
    finishedAt: null,
    cancelled: false,
    error: null,
    agents: [],
    ...overrides,
  };
}

describe('byteLabel', () => {
  it('scales to the unit a reader can judge', () => {
    expect(byteLabel(1024)).toBe('1.0 KB');
    expect(byteLabel(3_670_016)).toBe('3.5 MB');
    expect(byteLabel(52 * 1024 * 1024)).toBe('52 MB');
    expect(byteLabel(3 * 1024 ** 3)).toBe('3.0 GB');
  });

  it('says nothing rather than something wrong for an absent figure', () => {
    // A fresh install has no database file yet, and the shell reports zero
    // instead of failing a settings row over it.
    expect(byteLabel(0)).toBe('0 KB');
    expect(byteLabel(-1)).toBe('0 KB');
    expect(byteLabel(Number.NaN)).toBe('0 KB');
  });
});

describe('scanSummary', () => {
  it('distinguishes every outcome a pass can have', () => {
    const finishedAt = new Date(Date.now() - 60_000).toISOString();
    expect(scanSummary(null)).toBe('Nothing has been scanned yet.');
    expect(scanSummary(status({ running: true }))).toBe('Scanning now…');
    expect(scanSummary(status({ finishedAt, error: 'disk went away' }))).toBe(
      'The last scan did not finish.',
    );
    expect(scanSummary(status({ finishedAt, cancelled: true }))).toBe(
      'The last scan was stopped before it finished.',
    );
    expect(scanSummary(status({ finishedAt }))).toBe('Last scanned 1m ago.');
  });
});
