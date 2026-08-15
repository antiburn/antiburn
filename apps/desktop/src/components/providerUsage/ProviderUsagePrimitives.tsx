// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  siClaude,
  siCursor,
  siDeepseek,
  siGithubcopilot,
  siGooglegemini,
  siMistralai,
  siOpenrouter,
  siWindsurf,
  type SimpleIcon,
} from 'simple-icons';

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

/**
 * Canonical provider id → brand mark, from the CC0-licensed `simple-icons`
 * package. The same treatment, and the same rules, as the agent icons in
 * `lib/agentIcon` (deviations register D-18).
 *
 * **Where a vendor already has a mark on the session cards, this table uses
 * the same one.** Anthropic is drawn with the Claude mark rather than the
 * corporate one, because that is what a session row shows and a reader
 * scrolling from their sessions to their usage should not have to work out
 * that two marks mean one vendor. Cursor, Windsurf, and GitHub Copilot align
 * with `lib/agentIcon`'s registry for the same reason.
 *
 * Two absences are deliberate rather than pending:
 *
 * - **OpenAI.** The package carries no OpenAI mark; its nearest entry is
 *   `siOpenaigym`, which belongs to a different product. Wearing it would be
 *   the same mistake D-18 already refuses for Amp, where the package's `AMP`
 *   is Google's web framework. OpenAI keeps its letter.
 * - **xAI.** `siX` is the social network, not the model provider.
 *
 * Marks render in `currentColor` so the set reads as one in both themes, and
 * the use is nominative: identifying whose allowance a figure describes.
 */
const PROVIDER_MARKS: Record<string, SimpleIcon> = {
  anthropic: siClaude,
  github: siGithubcopilot,
  google: siGooglegemini,
  cursor: siCursor,
  windsurf: siWindsurf,
  openrouter: siOpenrouter,
  deepseek: siDeepseek,
  mistral: siMistralai,
};

export interface ProviderGlyphProps {
  displayName: string;
  /** Canonical provider id. Without it, every provider gets a letter. */
  provider?: string;
  /** Edge length in pixels. */
  size?: number;
  className?: string;
}

/**
 * A provider's mark, or its initial where no rights-cleared mark exists.
 *
 * Decorative either way — every caller names the provider in its own
 * accessible name — so it is hidden from assistive technology rather than read
 * out as a stray capital.
 */
export function ProviderGlyph({
  displayName,
  provider,
  size = 16,
  className = '',
}: ProviderGlyphProps) {
  const mark = provider ? PROVIDER_MARKS[provider] : undefined;
  if (mark) {
    return (
      <svg
        aria-hidden="true"
        viewBox="0 0 24 24"
        width={size}
        height={size}
        fill="currentColor"
        data-provider-icon={provider}
        className={`shrink-0 text-label-secondary ${className}`.trimEnd()}
      >
        <path d={mark.path} />
      </svg>
    );
  }
  return (
    <span
      aria-hidden="true"
      data-provider-icon="letter"
      style={{ width: size, height: size, fontSize: Math.round(size * 0.58) }}
      className={`inline-flex shrink-0 items-center justify-center rounded-[4px] bg-surface-secondary font-medium leading-none text-label-secondary ${className}`.trimEnd()}
    >
      {providerInitial(displayName)}
    </span>
  );
}

/** The brand mark for a provider, when one exists. Used by the ring. */
export function providerMarkPath(provider: string): string | undefined {
  return PROVIDER_MARKS[provider]?.path;
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
