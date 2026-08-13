// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { ProviderUsageState } from '../../lib/ipc';
import {
  providerInitial,
  usageStateLabel,
  usageStateToneClass,
} from '../../lib/presentation/providerUsage';

/**
 * The two marks the usage surfaces repeat: a provider's glyph and its state.
 *
 * Both are pure presentation, and both are deliberately quiet. A usage figure
 * that shouted would read as a budget, and there is no budget here to read.
 */

export interface ProviderGlyphProps {
  displayName: string;
  /** Edge length in pixels. */
  size?: number;
  className?: string;
}

/**
 * A provider's initial in a rounded tile.
 *
 * A letter, not a logo: provider artwork is licensed separately from this
 * application and none of it is bundled. The tile is decorative — every caller
 * names the provider in its own accessible name — so it is hidden from
 * assistive technology rather than read out as a stray capital.
 */
export function ProviderGlyph({ displayName, size = 16, className = '' }: ProviderGlyphProps) {
  return (
    <span
      aria-hidden="true"
      style={{ width: size, height: size, fontSize: Math.round(size * 0.58) }}
      className={`inline-flex shrink-0 items-center justify-center rounded-[4px] bg-surface-secondary font-medium leading-none text-label-secondary ${className}`.trimEnd()}
    >
      {providerInitial(displayName)}
    </span>
  );
}

export interface UsageStateBadgeProps {
  state: ProviderUsageState;
  className?: string;
}

/**
 * What kind of evidence produced a provider's figures.
 *
 * The word is visible rather than encoded in a color, so the distinction
 * between a priced estimate and a bare token count survives both a screen
 * reader and a monochrome display.
 */
export function UsageStateBadge({ state, className = '' }: UsageStateBadgeProps) {
  return (
    <span
      className={`inline-flex shrink-0 items-center rounded-full px-1.5 py-px type-caption font-medium leading-[13px] ${usageStateToneClass(
        state,
      )} ${className}`.trimEnd()}
    >
      {usageStateLabel(state)}
    </span>
  );
}
