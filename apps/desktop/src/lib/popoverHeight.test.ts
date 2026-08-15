// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  DEFAULT_POPOVER_HEIGHT,
  MAX_POPOVER_HEIGHT,
  POPOVER_HEIGHTS,
  popoverHeightFor,
  prefersReducedMotion,
} from './popoverHeight';

describe('popover heights', () => {
  it('holds every surface between the shell’s two clamps', () => {
    expect(DEFAULT_POPOVER_HEIGHT).toBe(700);
    expect(MAX_POPOVER_HEIGHT).toBe(780);
    for (const [surface, height] of Object.entries(POPOVER_HEIGHTS)) {
      expect(height, `${surface} is taller than the ceiling allows`).toBeLessThanOrEqual(
        MAX_POPOVER_HEIGHT,
      );
      // Nothing so short it cannot hold its own chrome; the shell clamps to
      // 320 and a value below that would silently become a different number.
      expect(height).toBeGreaterThanOrEqual(320);
    }
  });

  it('gives only the surface that outgrew the contract more than the contract', () => {
    // D-22: the ceiling is above the contract's height, and exactly one
    // surface uses it. Everything else stays where it was.
    expect(MAX_POPOVER_HEIGHT).toBeGreaterThan(DEFAULT_POPOVER_HEIGHT);
    expect(popoverHeightFor('usage')).toBe(MAX_POPOVER_HEIGHT);
    expect(popoverHeightFor('activity')).toBe(DEFAULT_POPOVER_HEIGHT);
    expect(popoverHeightFor('session')).toBe(DEFAULT_POPOVER_HEIGHT);
    expect(popoverHeightFor('onboarding')).toBeLessThan(DEFAULT_POPOVER_HEIGHT);
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
