// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  MAX_POPOVER_HEIGHT,
  POPOVER_HEIGHTS,
  popoverHeightFor,
  prefersReducedMotion,
} from './popoverHeight';

describe('popover heights', () => {
  it('holds every surface at or under the 700px ceiling', () => {
    expect(MAX_POPOVER_HEIGHT).toBe(700);
    for (const [surface, height] of Object.entries(POPOVER_HEIGHTS)) {
      expect(height, `${surface} is taller than the contract allows`).toBeLessThanOrEqual(
        MAX_POPOVER_HEIGHT,
      );
      // Nothing so short it cannot hold its own chrome; the shell clamps to
      // 320 and a value below that would silently become a different number.
      expect(height).toBeGreaterThanOrEqual(320);
    }
  });

  it('gives the content-heavy surfaces the full window and onboarding less', () => {
    expect(popoverHeightFor('activity')).toBe(MAX_POPOVER_HEIGHT);
    expect(popoverHeightFor('session')).toBe(MAX_POPOVER_HEIGHT);
    expect(popoverHeightFor('onboarding')).toBeLessThan(MAX_POPOVER_HEIGHT);
    expect(popoverHeightFor('usage')).toBeLessThan(MAX_POPOVER_HEIGHT);
  });
});

describe('prefersReducedMotion', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('follows the media query when the platform has one', () => {
    const matchMedia = vi.fn(() => ({ matches: true }));
    vi.stubGlobal('window', { ...globalThis.window, matchMedia });
    expect(prefersReducedMotion()).toBe(true);
    expect(matchMedia).toHaveBeenCalledWith('(prefers-reduced-motion: reduce)');
  });

  it('treats a platform without the query as "no preference"', () => {
    vi.stubGlobal('window', {});
    // The browser default, and the safe one: a missing query is not consent to
    // remove motion the reader never asked to lose.
    expect(prefersReducedMotion()).toBe(false);
  });
});
