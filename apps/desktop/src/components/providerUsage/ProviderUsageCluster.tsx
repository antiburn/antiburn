// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Settings } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';

import type { LiveUsageSummaryPayload, ProviderUsagePayload } from '../../lib/ipc';
import { EMPTY_LIVE_USAGE } from '../../lib/ipc';
import {
  headlineWindow,
  liveForProvider,
  liveWindowLabel,
  liveWindowValueLabel,
} from '../../lib/presentation/liveUsage';
import {
  providerInitial,
  providersForWindow,
  providerWindow,
  stalenessNote,
  usageStateLabel,
  usageValueLabel,
} from '../../lib/presentation/providerUsage';
import { TextRoll } from '../ui/TextRoll';
import { ProviderUsageDetail } from './ProviderUsageDetail';
import { ProviderGlyph, providerMark } from './ProviderUsagePrimitives';
import { UsageRing } from './UsageRing';

/** Chips shown before the rest collapse into a single overflow affordance. */
export const DEFAULT_MAX_CHIPS = 3;

export interface ProviderUsageClusterProps {
  providers: readonly ProviderUsagePayload[];
  /** The provider's own limit figures, when a source could prove any. */
  live?: LiveUsageSummaryPayload;
  /**
   * The instant countdowns are measured from. Defaults to when the shell
   * collected the snapshot — a render must not read the clock, and the
   * countdown agrees with the reading it sits under this way.
   */
  now?: number;
  /** Open the full Usage view. */
  onViewAll: () => void;
  /** Open the standalone Settings window (the footer's right-hand gear). */
  onOpenSettings: () => void;
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
 * There are two ways in, and they are not the same thing. **Hovering** a chip
 * opens the panel after a short delay and closes it a shorter one after the
 * pointer leaves — the delays exist so a pointer crossing the footer on its
 * way somewhere else does not strobe three panels, and so the diagonal from
 * chip to panel is forgiving. **Clicking or tabbing to** a chip opens the same
 * panel deliberately, and it stays until dismissed.
 *
 * Only the deliberate path takes the obligations of a dialog: focus moves into
 * the panel, Tab is held inside it, and focus returns to the chip on close. A
 * hover-opened panel does none of that and is not `aria-modal`, because
 * yanking focus out from under a pointer that merely passed over a chip is
 * hostile — and a modal nobody asked for is worse than a disclosure.
 *
 * Hover is an enhancement on top of the click path, never a replacement for
 * it: a touch screen has no hover, and neither does a keyboard. Focus
 * deliberately does *not* open the panel — Enter or Space on a focused chip
 * already fires a click, and opening on focus as well would reopen the panel
 * the instant a close handed focus back to the chip.
 */

/**
 * How long a pointer must rest on a chip before its panel opens, and how long
 * it may be away before the panel closes.
 *
 * Open is the longer of the two: a pointer crossing the footer on its way to
 * the gear should not light up three panels behind it. Close is shorter but
 * not zero, so the diagonal from a chip to the panel above it is forgiving.
 */
const HOVER_OPEN_MS = 200;
const HOVER_CLOSE_MS = 140;

/** Everything inside the panel a Tab can reach. */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
export function ProviderUsageCluster({
  providers,
  live = EMPTY_LIVE_USAGE,
  now,
  onViewAll,
  onOpenSettings,
  maxVisible = DEFAULT_MAX_CHIPS,
}: ProviderUsageClusterProps) {
  const at = now ?? (Date.parse(live.generatedAt) || 0);
  const [openProvider, setOpenProvider] = useState<string | null>(null);
  /**
   * How the open panel was opened. `pointer` means a click or a key — the
   * deliberate path, which takes the dialog obligations. `hover` means the
   * pointer merely arrived, which takes none of them.
   */
  const [openedBy, setOpenedBy] = useState<'hover' | 'pointer'>('pointer');
  /** Mirrors `openedBy` so a pending timer reads it without re-subscribing. */
  const openedByRef = useRef(openedBy);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  /** The chip that opened the panel, so focus can be handed back to it. */
  const invokerRef = useRef<HTMLElement | null>(null);
  /** The pending hover open or close, so either can be called off. */
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const panelId = useId();
  const headingId = `${panelId}-heading`;

  const cancelHover = useCallback(() => {
    if (hoverTimer.current != null) {
      clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  }, []);

  /** Open on hover, once the pointer has stayed long enough to mean it. */
  const hoverOpen = useCallback(
    (provider: string) => {
      cancelHover();
      hoverTimer.current = setTimeout(() => {
        setOpenProvider((current) => {
          // A deliberately-opened panel is not replaced by a passing pointer.
          if (current != null && openedByRef.current === 'pointer') return current;
          setOpenedBy('hover');
          return provider;
        });
      }, HOVER_OPEN_MS);
    },
    [cancelHover],
  );

  /** Close on hover-out, unless the reader opened it on purpose. */
  const hoverClose = useCallback(() => {
    cancelHover();
    hoverTimer.current = setTimeout(() => {
      if (openedByRef.current === 'pointer') return;
      setOpenProvider(null);
    }, HOVER_CLOSE_MS);
  }, [cancelHover]);

  const today = providersForWindow(providers, 'today');
  const visible = today.slice(0, maxVisible);
  const overflow = today.length - visible.length;
  const open = visible.find((provider) => provider.provider === openProvider) ?? null;

  const close = useCallback(() => {
    cancelHover();
    setOpenProvider(null);
    // Returned synchronously rather than in an effect: after the panel is gone
    // there is no element to compute this from, and focus would fall to the
    // top of the document. Only the deliberate path put focus inside, so only
    // it has focus to give back — a hover close must leave the pointer's own
    // focus exactly where it was.
    if (openedByRef.current === 'pointer') invokerRef.current?.focus();
    invokerRef.current = null;
  }, [cancelHover]);

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
      // Tab is only held inside a panel the reader opened on purpose. A
      // hover panel is not modal and must not capture the key.
      if (event.key !== 'Tab' || openedByRef.current !== 'pointer') return;

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
    if (!open || openedBy !== 'pointer') return;
    const panel = panelRef.current;
    if (!panel) return;
    const target = panel.querySelector<HTMLElement>(FOCUSABLE) ?? panel;
    target.focus();
  }, [open, openedBy]);

  // Synced in an effect rather than assigned during render: a ref written
  // while rendering is a value React is entitled to discard.
  useEffect(() => {
    openedByRef.current = openedBy;
  }, [openedBy]);

  // A pending timer outliving the component would set state on nothing.
  useEffect(() => cancelHover, [cancelHover]);

  const viewAll = () => {
    close();
    onViewAll();
  };

  return (
    <div
      ref={rootRef}
      data-testid="provider-usage-cluster"
      className="relative flex h-11 shrink-0 items-center gap-1 border-t border-separator px-2"
    >
      {visible.map((provider) => {
        const window = providerWindow(provider, 'today');
        const value = usageValueLabel(window);
        const stale = stalenessNote(provider);
        const isOpen = open?.provider === provider.provider;
        // The ring is drawn only where the provider stated a percentage. The
        // spend figure beside it has no denominator, so borrowing the shape
        // for it would mean something here that it does not mean. Which window
        // the ring shows is `headlineWindow`'s decision, and it is the
        // account-wide one rather than the fullest — see its doc.
        const limits = liveForProvider(live, provider.provider);
        const headline = limits ? headlineWindow(limits) : null;
        return (
          <button
            key={provider.provider}
            type="button"
            aria-haspopup="dialog"
            aria-expanded={isOpen}
            aria-controls={isOpen ? panelId : undefined}
            // The chip shows a glyph and a number; the name, the window, and
            // what kind of figure it is have to live in the accessible name or
            // they are lost. The ring adds a second fact, so it adds a clause:
            // a shape with no text is invisible to a screen reader.
            aria-label={`${provider.displayName}, ${value} today, ${usageStateLabel(
              provider.state,
            ).toLocaleLowerCase()}${
              headline
                ? `, ${liveWindowLabel(headline).toLocaleLowerCase()} ${liveWindowValueLabel(
                    headline,
                  ).toLocaleLowerCase()}`
                : ''
            }${stale ? `, ${stale.toLocaleLowerCase()}` : ''}`}
            onPointerEnter={(event) => {
              // Touch reports a pointer enter immediately before the click it
              // is about to fire; opening on it would make the tap a toggle
              // that opens and then closes.
              if (event.pointerType === 'touch') return;
              hoverOpen(provider.provider);
            }}
            onPointerLeave={(event) => {
              if (event.pointerType === 'touch') return;
              hoverClose();
            }}
            onClick={(event) => {
              cancelHover();
              if (openProvider === provider.provider && openedBy === 'pointer') {
                close();
                return;
              }
              invokerRef.current = event.currentTarget;
              setOpenedBy('pointer');
              setOpenProvider(provider.provider);
            }}
            className={`inline-flex h-7 shrink-0 items-center gap-1.5 rounded-control px-1.5 type-caption tabular-nums leading-none text-label-secondary hover:bg-surface-hover ${
              isOpen ? 'bg-surface-hover' : ''
            }`.trimEnd()}
          >
            {headline ? (
              // The ring carries the provider's initial rather than replacing
              // it: a chip that says how full something is without saying
              // whose is a worse trade than the ring is worth.
              <UsageRing
                percent={headline.usedPercent}
                mark={providerMark(provider.provider)}
                glyph={providerInitial(provider.displayName)}
                size={22}
                className="shrink-0 text-label-secondary"
              />
            ) : (
              <ProviderGlyph
                displayName={provider.displayName}
                provider={provider.provider}
                size={18}
              />
            )}
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
          className="inline-flex h-7 shrink-0 items-center rounded-control px-1 type-caption text-label-tertiary hover:bg-surface-hover"
        >
          +{overflow}
        </button>
      )}

      <button
        type="button"
        onClick={onOpenSettings}
        aria-label="Open settings"
        className="-mr-0.5 ml-auto inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-control text-label-secondary hover:bg-surface-hover"
      >
        <Settings size={14} strokeWidth={1.75} aria-hidden="true" />
      </button>

      {open && (
        <div
          ref={panelRef}
          id={panelId}
          role="dialog"
          // Modal only when the reader opened it on purpose. A hover panel
          // that claimed to be modal would be promising containment that is
          // deliberately not there.
          aria-modal={openedBy === 'pointer' ? 'true' : undefined}
          aria-labelledby={headingId}
          // Keeps the panel alive while the pointer travels into it, and
          // starts the close when it leaves — so the panel is as hoverable as
          // the chip that opened it.
          onPointerEnter={cancelHover}
          onPointerLeave={(event) => {
            if (event.pointerType === 'touch') return;
            hoverClose();
          }}
          // Focusable so the dialog itself can hold focus when it contains no
          // control; never in the Tab order.
          tabIndex={-1}
          className="ui-anchored-panel absolute bottom-full left-2 right-2 mb-1.5 p-3 outline-none"
        >
          <ProviderUsageDetail
            provider={open}
            live={liveForProvider(live, open.provider)}
            now={at}
            headingId={headingId}
            onViewAll={viewAll}
          />
        </div>
      )}
    </div>
  );
}
