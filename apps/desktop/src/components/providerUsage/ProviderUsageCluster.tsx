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
 *
 * The panel is a real `role="dialog"`, so it carries the obligations of one:
 * focus moves into it on open, Tab is held inside it, and focus returns to the
 * chip that opened it on close. A dialog a keyboard user is not *in* is worse
 * than a plain disclosure, because the role promises containment that is not
 * there.
 */

/** Everything inside the panel a Tab can reach. */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
export function ProviderUsageCluster({
  providers,
  onViewAll,
  maxVisible = DEFAULT_MAX_CHIPS,
}: ProviderUsageClusterProps) {
  const [openProvider, setOpenProvider] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  /** The chip that opened the panel, so focus can be handed back to it. */
  const invokerRef = useRef<HTMLElement | null>(null);
  const panelId = useId();
  const headingId = `${panelId}-heading`;

  const today = providersForWindow(providers, 'today');
  const visible = today.slice(0, maxVisible);
  const overflow = today.length - visible.length;
  const open = visible.find((provider) => provider.provider === openProvider) ?? null;

  const close = useCallback(() => {
    setOpenProvider(null);
    // Returned synchronously rather than in an effect: after the panel is gone
    // there is no element to compute this from, and focus would fall to the
    // top of the document.
    invokerRef.current?.focus();
    invokerRef.current = null;
  }, []);

  // Dismissal is a genuine synchronization with the document: a pointer press
  // anywhere outside the footer closes the panel, and Escape closes it from
  // the keyboard. Both listeners exist only while a panel is open.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        // Claimed, so the popover's own Escape handler does not also dismiss
        // the whole window: one key press closes one thing.
        event.preventDefault();
        close();
        return;
      }
      if (event.key !== 'Tab') return;

      const panel = panelRef.current;
      if (!panel) return;
      const focusable = Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE));
      // A panel with nothing to focus still holds the key, or Tab would walk
      // out of a dialog the reader has not dismissed.
      if (focusable.length === 0) {
        event.preventDefault();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey && (active === first || !panel.contains(active))) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && (active === last || !panel.contains(active))) {
        event.preventDefault();
        first?.focus();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open, close]);

  // Move focus into the dialog on open. The panel itself is the target when it
  // has no control of its own, so the reader is at least *inside* the thing
  // their key presses now apply to.
  useEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    if (!panel) return;
    const target = panel.querySelector<HTMLElement>(FOCUSABLE) ?? panel;
    target.focus();
  }, [open]);

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
            onClick={(event) => {
              if (openProvider === provider.provider) {
                close();
                return;
              }
              invokerRef.current = event.currentTarget;
              setOpenProvider(provider.provider);
            }}
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
          ref={panelRef}
          id={panelId}
          role="dialog"
          aria-modal="true"
          aria-labelledby={headingId}
          // Focusable so the dialog itself can hold focus when it contains no
          // control; never in the Tab order.
          tabIndex={-1}
          className="ui-anchored-panel absolute bottom-full left-2 right-2 mb-1.5 p-3 outline-none"
        >
          <ProviderUsageDetail provider={open} headingId={headingId} onViewAll={viewAll} />
        </div>
      )}
    </div>
  );
}
