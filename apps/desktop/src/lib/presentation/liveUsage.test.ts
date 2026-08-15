// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from 'vitest';

import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from '../ipc';
import {
  liveAuthNote,
  liveExtraUsageLabel,
  liveForProvider,
  liveFreshnessToneClass,
  liveResetLabel,
  liveSourceNote,
  liveStalenessNote,
  liveSupportDescription,
  liveSupportLabel,
  liveWindowElapsed,
  liveWindowLabel,
  liveWindowValueLabel,
  liveWindows,
} from './liveUsage';

const NOW = Date.parse('2027-01-15T12:00:00Z');

function window(overrides: Partial<LiveUsageWindowPayload> = {}): LiveUsageWindowPayload {
  return {
    id: 'five-hour',
    role: 'primaryShort',
    kind: 'rolling',
    scopeModel: null,
    usedPercent: 81,
    startsAt: null,
    resetsAt: '2027-01-15T14:30:00Z',
    ...overrides,
  };
}

function provider(overrides: Partial<LiveProviderUsagePayload> = {}): LiveProviderUsagePayload {
  return {
    provider: 'anthropic',
    displayName: 'Anthropic',
    support: 'live',
    freshness: 'fresh',
    sourceLabel: "Claude's cached usage",
    // Relative to the real clock, not to `NOW`: `relativeTime` phrases an age
    // against the moment it runs, while the window helpers take their `now` as
    // an argument. Only the former needs a real-world stamp.
    observedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
    windows: [window()],
    extraUsage: null,
    ...overrides,
  };
}

function summary(overrides: Partial<LiveUsageSummaryPayload> = {}): LiveUsageSummaryPayload {
  return { providers: [provider()], errors: [], generatedAt: '', ...overrides };
}

describe('capability labels', () => {
  it('names every support tier, including the ones no source produces yet', () => {
    expect(liveSupportLabel('live')).toBe('Live');
    expect(liveSupportLabel('estimated')).toBe('Estimated');
    expect(liveSupportLabel('observed')).toBe('Activity only');
    expect(liveSupportLabel('detected')).toBe('Unavailable');
  });

  it('describes a live figure as the provider’s own, never as ours', () => {
    expect(liveSupportDescription('live')).toMatch(/your provider stated/i);
    expect(liveSupportDescription('estimated')).toMatch(/rather than stated/i);
  });
});

describe('window labels', () => {
  it('names a model-scoped weekly limit after its model', () => {
    // Otherwise it is indistinguishable from the account-wide weekly limit
    // sitting directly above it.
    expect(liveWindowLabel(window({ scopeModel: 'Some Model', role: 'supplemental' }))).toBe(
      'Some Model weekly limit',
    );
    expect(liveWindowLabel(window({ role: 'primaryShort' }))).toBe('5-hour limit');
    expect(liveWindowLabel(window({ role: 'primaryLong' }))).toBe('Weekly limit');
  });

  it('falls back to a neutral name rather than guessing at an unknown role', () => {
    expect(liveWindowLabel(window({ role: 'some_future_limit', kind: 'some_future_limit' }))).toBe(
      'Usage limit',
    );
  });

  it('reads an absent percentage as unknown, never as zero', () => {
    // An empty meter is a claim, and this one would be a claim nobody made.
    expect(liveWindowValueLabel(window({ usedPercent: null }))).toBe('Unknown');
    expect(liveWindowValueLabel(window({ usedPercent: 0 }))).toBe('0% used');
    expect(liveWindowValueLabel(window({ usedPercent: 80.6 }))).toBe('81% used');
  });
});

describe('the elapsed marker', () => {
  it('takes the period from the window’s own name when the provider states no start', () => {
    // The provider states a reset but no start, so without this the marker
    // would never appear at all. `five-hour` is not a guess about the period:
    // the id states it, and the start is five hours before the reset.
    expect(
      liveWindowElapsed(
        window({ id: 'five-hour', startsAt: null, resetsAt: '2027-01-15T14:00:00Z' }),
        NOW,
      ),
    ).toBeCloseTo(0.6);
  });

  it('refuses to guess a period the window’s name does not state', () => {
    // "Seven days before the reset" would be a guess dressed as a
    // measurement — a weekly boundary is the provider's own.
    expect(
      liveWindowElapsed(window({ id: 'seven-day', role: 'primaryLong', startsAt: null }), NOW),
    ).toBeNull();
    expect(liveWindowElapsed(window({ id: 'weekly-some-model', startsAt: null }), NOW)).toBeNull();
    expect(liveWindowElapsed(window({ resetsAt: null }), NOW)).toBeNull();
  });

  it('prefers a stated start over an implied one', () => {
    expect(
      liveWindowElapsed(
        window({
          id: 'five-hour',
          startsAt: '2027-01-15T11:00:00Z',
          resetsAt: '2027-01-15T13:00:00Z',
        }),
        NOW,
      ),
    ).toBeCloseTo(0.5);
  });

  it('measures the clock’s progress through the provider’s own period', () => {
    const marked = window({
      id: 'seven-day',
      startsAt: '2027-01-15T10:00:00Z',
      resetsAt: '2027-01-15T14:00:00Z',
    });
    // Two hours into a four-hour window.
    expect(liveWindowElapsed(marked, NOW)).toBeCloseTo(0.5);
  });

  it('clamps to the period rather than reporting past its end', () => {
    const done = window({
      id: 'seven-day',
      startsAt: '2027-01-15T06:00:00Z',
      resetsAt: '2027-01-15T08:00:00Z',
    });
    expect(liveWindowElapsed(done, NOW)).toBe(1);
  });

  it('refuses a window whose dates make no sense', () => {
    const backwards = window({
      id: 'seven-day',
      startsAt: '2027-01-15T14:00:00Z',
      resetsAt: '2027-01-15T10:00:00Z',
    });
    expect(liveWindowElapsed(backwards, NOW)).toBeNull();
    expect(liveWindowElapsed(window({ id: 'seven-day', startsAt: 'tuesday' }), NOW)).toBeNull();
  });
});

describe('reset labels', () => {
  it('counts down within the day and names the day beyond it', () => {
    expect(liveResetLabel(window({ resetsAt: '2027-01-15T14:30:00Z' }), NOW)).toBe(
      'Resets in 2h 30m',
    );
    expect(liveResetLabel(window({ resetsAt: '2027-01-15T12:45:00Z' }), NOW)).toBe(
      'Resets in 45m',
    );
    // Beyond a day, "resets in 76h" is not a sentence anyone reads as a time.
    expect(liveResetLabel(window({ resetsAt: '2027-01-19T18:00:00Z' }), NOW)).toMatch(
      /^Resets \w{3} /,
    );
  });

  it('says so when the provider gave no reset, and distinguishes that from a passed one', () => {
    expect(liveResetLabel(window({ resetsAt: null }), NOW)).toBe('Reset unavailable');
    expect(liveResetLabel(window({ resetsAt: 'not a date' }), NOW)).toBe('Reset unavailable');
    // Past, because the window rolls on the provider's clock and not ours.
    expect(liveResetLabel(window({ resetsAt: '2027-01-15T11:00:00Z' }), NOW)).toBe(
      'Reset pending',
    );
  });
});

describe('provenance', () => {
  it('says what kind of reading it is and when it was stated', () => {
    expect(liveSourceNote(provider())).toBe('Live · stated 5m ago');
  });

  it('marks a stale reading and leaves a fresh one alone', () => {
    expect(liveFreshnessToneClass('stale')).toContain('orange');
    expect(liveFreshnessToneClass('fresh')).not.toContain('orange');
    expect(liveStalenessNote(provider())).toBeNull();
    expect(liveStalenessNote(provider({ freshness: 'stale' }))).toMatch(/may have moved since/);
  });
});

describe('ordering and lookup', () => {
  it('puts the primary windows first and keeps the provider’s order within a rank', () => {
    const ordered = liveWindows(
      provider({
        windows: [
          window({ id: 'weekly-a', role: 'supplemental', scopeModel: 'A' }),
          window({ id: 'seven-day', role: 'primaryLong' }),
          window({ id: 'weekly-b', role: 'supplemental', scopeModel: 'B' }),
          window({ id: 'five-hour', role: 'primaryShort' }),
        ],
      }),
    );
    expect(ordered.map((entry) => entry.id)).toEqual([
      'five-hour',
      'seven-day',
      'weekly-a',
      'weekly-b',
    ]);
  });

  it('joins to the estimate payload by provider id', () => {
    expect(liveForProvider(summary(), 'anthropic')?.displayName).toBe('Anthropic');
    expect(liveForProvider(summary(), 'openai')).toBeNull();
  });
});

describe('the failure surface', () => {
  it('banners only the failure a reader can act on', () => {
    expect(liveAuthNote(summary())).toBeNull();
    // A rate limit passes on its own and an unreadable file is usually a
    // missing agent; neither is worth interrupting someone over.
    expect(
      liveAuthNote(summary({ errors: [{ source: 's', category: 'rateLimited' }] })),
    ).toBeNull();
    expect(
      liveAuthNote(summary({ errors: [{ source: 's', category: 'unavailable' }] })),
    ).toBeNull();
    expect(
      liveAuthNote(summary({ errors: [{ source: 's', category: 'authentication' }] })),
    ).toMatch(/sign in again/i);
  });
});

describe('extra usage', () => {
  it('stays silent about a meter the reader has already turned off', () => {
    expect(liveExtraUsageLabel(provider())).toBeNull();
    expect(
      liveExtraUsageLabel(
        provider({
          extraUsage: {
            enabled: false,
            usedPercent: null,
            used: null,
            remaining: null,
            limit: null,
            currency: null,
          },
        }),
      ),
    ).toBeNull();
  });

  it('leads with the percentage, falls back to the amount, then to bare presence', () => {
    const extra = {
      enabled: true,
      usedPercent: 25,
      used: 5,
      remaining: 15,
      limit: 20,
      currency: 'USD',
    };
    expect(liveExtraUsageLabel(provider({ extraUsage: extra }))).toBe('25% of extra usage');
    expect(
      liveExtraUsageLabel(provider({ extraUsage: { ...extra, usedPercent: null } })),
    ).toBe('5.00 USD of extra usage');
    expect(
      liveExtraUsageLabel(
        provider({ extraUsage: { ...extra, usedPercent: null, used: null, currency: null } }),
      ),
    ).toBe('Extra usage is on');
  });
});
