// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useEffect, useId, useRef, useState } from 'react';

import type { ProviderUsagePayload } from '../../lib/ipc';
import {
  providersForWindow,
  providerWindow,
  stalenessNote,
  usageStateLabel,
  usageValueLabel,
} from '../../lib/presentation/providerUsage';
import { TextRoll } from '../ui/TextRoll';
import { ProviderUsageDetail } from './ProviderUsageDetail';
import { ProviderGlyph } from './ProviderUsagePrimitives';

/** Chips shown before the rest collapse into a single overflow affordance. */
export const DEFAULT_MAX_CHIPS = 3;

export interface ProviderUsageClusterProps {
  providers: readonly ProviderUsagePayload[];
  /** Open the full Usage view. */
  onViewAll: () => void;
  maxVisible?: number;
}

/**
 * The popover's usage footer: one compact chip per provider used today, an
 * overflow count, and the way through to the full Usage view.
 *
 * Chips are drawn from *today* only. A provider the reader has not touched
 * since yesterday is not shown a dash here — it is simply not in the footer,
 * and the Usage view is where the wider windows live. That keeps the resting
 * state of the popover a statement about right now.
 *
 * Clicking a chip opens a panel anchored above the footer. It is positioned by
 * this component rather than portalled, because the popover window is 380px of
 * fixed chrome: a portalled surface would have nowhere to escape to, and a
 * collision-aware library would only be re-deriving "sit above the footer".
 */
export function ProviderUsageCluster({
  providers,
  onViewAll,
  maxVisible = DEFAULT_MAX_CHIPS,
}: ProviderUsageClusterProps) {
  const [openProvider, setOpenProvider] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const panelId = useId();
  const headingId = `${panelId}-heading`;

  const today = providersForWindow(providers, 'today');
  const visible = today.slice(0, maxVisible);
  const overflow = today.length - visible.length;
  const open = visible.find((provider) => provider.provider === openProvider) ?? null;

  const close = useCallback(() => setOpenProvider(null), []);

  // Dismissal is a genuine synchronization with the document: a pointer press
  // anywhere outside the footer closes the panel, and Escape closes it from
  // the keyboard. Both listeners exist only while a panel is open.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, close]);

  const viewAll = () => {
    close();
    onViewAll();
  };

  return (
    <div
      ref={rootRef}
      data-testid="provider-usage-cluster"
      className="relative flex h-9 shrink-0 items-center gap-1 border-t border-separator px-2"
    >
      {visible.map((provider) => {
        const window = providerWindow(provider, 'today');
        const value = usageValueLabel(window);
        const stale = stalenessNote(provider);
        const isOpen = open?.provider === provider.provider;
        return (
          <button
            key={provider.provider}
            type="button"
            aria-haspopup="dialog"
            aria-expanded={isOpen}
            aria-controls={isOpen ? panelId : undefined}
            // The chip shows a glyph and a number; the name, the window, and
            // what kind of figure it is have to live in the accessible name or
            // they are lost.
            aria-label={`${provider.displayName}, ${value} today, ${usageStateLabel(
              provider.state,
            ).toLocaleLowerCase()}${stale ? `, ${stale.toLocaleLowerCase()}` : ''}`}
            onClick={() =>
              setOpenProvider((current) =>
                current === provider.provider ? null : provider.provider,
              )
            }
            className={`inline-flex h-6 shrink-0 items-center gap-1 rounded-control px-1.5 type-caption tabular-nums leading-none text-label-secondary hover:bg-surface-hover ${
              isOpen ? 'bg-surface-hover' : ''
            }`.trimEnd()}
          >
            <ProviderGlyph displayName={provider.displayName} size={14} />
            <TextRoll text={value} />
          </button>
        );
      })}

      {today.length === 0 && (
        <span className="type-caption text-label-tertiary">No provider usage today</span>
      )}

      {overflow > 0 && (
        <button
          type="button"
          onClick={viewAll}
          aria-label={`Show ${overflow} more provider${overflow === 1 ? '' : 's'}`}
          className="inline-flex h-6 shrink-0 items-center rounded-control px-1 type-caption text-label-tertiary hover:bg-surface-hover"
        >
          +{overflow}
        </button>
      )}

      <button
        type="button"
        onClick={viewAll}
        className="ml-auto inline-flex h-6 shrink-0 items-center rounded-control px-1.5 type-caption text-label-secondary hover:bg-surface-hover"
      >
        Usage
      </button>

      {open && (
        <div
          id={panelId}
          role="dialog"
          aria-labelledby={headingId}
          className="ui-anchored-panel absolute bottom-full left-2 right-2 mb-1.5 p-3"
        >
          <ProviderUsageDetail provider={open} headingId={headingId} onViewAll={viewAll} />
        </div>
      )}
    </div>
  );
}
