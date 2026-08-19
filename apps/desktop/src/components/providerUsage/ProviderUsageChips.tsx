// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useId, useRef, useState } from "react"
import { flushSync } from "react-dom"

import { cn } from "../../lib/cn"
import type { LiveUsageSummaryPayload, ProviderUsagePayload } from "../../lib/ipc"
import { EMPTY_LIVE_USAGE } from "../../lib/ipc"
import {
  liveWindowLabel,
  liveWindows,
  liveWindowValueLabel,
  maxLiveUsedPercent,
} from "../../lib/presentation/liveUsage"
import {
  providerInitial,
  stalenessNote,
  usageStateLabel,
} from "../../lib/presentation/providerUsage"
import { useDialogDismissal } from "../../lib/useDialogDismissal"
import { useHoverIntent } from "../../lib/useHoverIntent"
import { TextRoll } from "../ui/TextRoll"
import { ProviderUsageDetail } from "./ProviderUsageDetail"
import { ProviderGlyph, providerMark } from "./ProviderUsagePrimitives"
import { UsageRing } from "./UsageRing"

/** Chips shown before the rest collapse into a single overflow affordance. */
const DEFAULT_MAX_CHIPS = 3

interface ProviderUsageChipsProps {
  /**
   * The reader's own local spend and sessions. No longer what selects which
   * chips appear — see `live` below — but still what a chip's staleness note
   * comes from, and what its panel shows beneath the provider's own limits,
   * for a provider this device has actually run something through.
   */
  providers: readonly ProviderUsagePayload[]
  /** The provider's own limit figures, when a source could prove any. */
  live?: LiveUsageSummaryPayload
  /**
   * The instant countdowns are measured from. Defaults to when the shell
   * collected the snapshot — a render must not read the clock, and the
   * countdown agrees with the reading it sits under this way.
   */
  now?: number
  /** Open the full Usage view. */
  onViewAll: () => void
  maxVisible?: number
  /**
   * Which way the anchored detail panel opens. `"up"` sits the panel above
   * the chip row — for a row that lives at the bottom of its container.
   * `"down"` sits it below — for a row that lives at the top of one. There is
   * no collision detection because there does not need to be one: the popover
   * is a fixed 380px of chrome, so a caller near the bottom asks for `"up"`
   * and a caller near the top asks for `"down"`, and that is the whole
   * decision.
   */
  panelAnchor?: "up" | "down"
  className?: string
}

/**
 * One compact chip per provider with a live limit reading, an overflow
 * count, and the way through to the full Usage view.
 *
 * Chips are drawn from the *live* payload, not local spend: a chip shows a
 * percentage now, so it is selected the same way the expanded section above
 * it picks its subsections — any provider with at least one live window —
 * and the two always agree on which providers appear. A provider with local
 * spend but no live reading is not shown a dash here — it is simply not in
 * the row, and the Usage view is where local-only providers live. A provider
 * with a live reading but no local spend (nothing was ever run through it on
 * this device) still gets a chip; its panel just has no spend half to show.
 *
 * Clicking a chip opens a panel anchored to this row. It is positioned by
 * this component rather than portalled, because the popover window is 380px of
 * fixed chrome: a portalled surface would have nowhere to escape to, and a
 * collision-aware library would only be re-deriving "sit beside the row".
 *
 * There are two ways in, and they are not the same thing. **Hovering** a chip
 * opens the panel after a short delay and closes it a shorter one after the
 * pointer leaves — the delays exist so a pointer crossing the row on its way
 * somewhere else does not strobe three panels, and so the diagonal from chip
 * to panel is forgiving. **Clicking or tabbing to** a chip opens the same
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
 * Open is the longer of the two: a pointer crossing the row on its way to
 * somewhere else should not light up three panels behind it. Close is shorter
 * but not zero, so the diagonal from a chip to the panel beside it is
 * forgiving.
 */
const HOVER_OPEN_MS = 200
const HOVER_CLOSE_MS = 140

/** Everything inside the panel a Tab can reach. */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

export function ProviderUsageChips({
  providers,
  live = EMPTY_LIVE_USAGE,
  now,
  onViewAll,
  maxVisible = DEFAULT_MAX_CHIPS,
  panelAnchor = "up",
  className = "",
}: ProviderUsageChipsProps) {
  const at = now ?? (Date.parse(live.generatedAt) || 0)
  const [openProvider, setOpenProvider] = useState<string | null>(null)
  /**
   * How the open panel was opened. `pointer` means a click or a key — the
   * deliberate path, which takes the dialog obligations. `hover` means the
   * pointer merely arrived, which takes none of them.
   */
  const [openedBy, setOpenedBy] = useState<"hover" | "pointer">("pointer")
  /** Mirrors `openedBy` so a pending timer reads it without re-subscribing. */
  const openedByRef = useRef(openedBy)
  // Written directly during render rather than synced in an effect: this ref
  // is only ever read from timers and event handlers, never during render, so
  // there is no tearing to guard against. See useGlobalKeydown.ts for the
  // same "latest ref" pattern.
  // eslint-disable-next-line react-hooks/refs
  openedByRef.current = openedBy
  const rootRef = useRef<HTMLDivElement | null>(null)
  const panelRef = useRef<HTMLDivElement | null>(null)
  /** The chip that opened the panel, so focus can be handed back to it. */
  const invokerRef = useRef<HTMLElement | null>(null)
  /** The pending hover open or close, so either can be called off. */
  const { schedule: scheduleHover, cancel: cancelHover } = useHoverIntent()
  const panelId = useId()
  const headingId = `${panelId}-heading`

  /** Open on hover, once the pointer has stayed long enough to mean it. */
  const hoverOpen = useCallback(
    (provider: string) => {
      scheduleHover(() => {
        setOpenProvider((current) => {
          // A deliberately-opened panel is not replaced by a passing pointer.
          if (current != null && openedByRef.current === "pointer") return current
          setOpenedBy("hover")
          return provider
        })
      }, HOVER_OPEN_MS)
    },
    [scheduleHover],
  )

  /** Close on hover-out, unless the reader opened it on purpose. */
  const hoverClose = useCallback(() => {
    scheduleHover(() => {
      if (openedByRef.current === "pointer") return
      setOpenProvider(null)
    }, HOVER_CLOSE_MS)
  }, [scheduleHover])

  // Same predicate the expanded section filters its subsections with, so
  // collapsed and expanded never disagree about which providers are showing
  // anything.
  const limited = live.providers.filter((provider) => liveWindows(provider).length > 0)
  const visible = limited.slice(0, maxVisible)
  const overflow = limited.length - visible.length
  const open = visible.find((provider) => provider.provider === openProvider) ?? null
  /** The reader's own local spend for one provider id, when there is any. */
  const spendFor = (id: string) =>
    providers.find((provider) => provider.provider === id) ?? null

  const close = useCallback(() => {
    cancelHover()
    setOpenProvider(null)
    // Returned synchronously rather than in an effect: after the panel is gone
    // there is no element to compute this from, and focus would fall to the
    // top of the document. Only the deliberate path put focus inside, so only
    // it has focus to give back — a hover close must leave the pointer's own
    // focus exactly where it was.
    if (openedByRef.current === "pointer") invokerRef.current?.focus()
    invokerRef.current = null
  }, [cancelHover])

  // Dismissal is a genuine synchronization with the document: a pointer press
  // anywhere outside the row closes the panel, and Escape closes it from the
  // keyboard. Both listeners exist only while a panel is open. Tab is
  // additionally held inside the panel while it was opened on purpose.
  useDialogDismissal({
    active: !!open,
    containerRef: rootRef,
    trapRef: panelRef,
    trapFocus: openedBy === "pointer",
    focusableSelector: FOCUSABLE,
    onDismiss: close,
  })

  const viewAll = () => {
    close()
    onViewAll()
  }

  return (
    <div
      ref={rootRef}
      data-testid="provider-usage-chips"
      className={cn("relative flex min-w-0 items-center gap-1", className)}
    >
      {visible.map((limits) => {
        // Every provider that reaches this row is selected *because* it has
        // live windows — see `limited` above — but it may or may not have
        // ever produced local spend on this device. Where it has, the
        // staleness note and the panel's spend half both come from that.
        const spend = spendFor(limits.provider)
        const stale = spend ? stalenessNote(spend) : null
        const isOpen = open?.provider === limits.provider
        const windows = liveWindows(limits)
        // The ring shows the fullest live window, not the account-wide one —
        // see `maxLiveUsedPercent`'s doc for why a compact glance prefers the
        // worst case over which window happens to be the account's own.
        const maxPercent = maxLiveUsedPercent(limits)
        // A dollar figure here would be the local estimate, which has no
        // denominator to read against — showing it beside a ring drawn from
        // the provider's own percentage would imply a relationship that is
        // not there. So the chip shows only what the provider itself stated:
        // every live window's percentage, in the same order the panel lists
        // them.
        const value = windows.map((window) => liveWindowValueLabel(window)).join(" / ")
        return (
          <button
            key={limits.provider}
            type="button"
            aria-haspopup="dialog"
            aria-expanded={isOpen}
            aria-controls={isOpen ? panelId : undefined}
            // The chip shows a glyph and a number per live window; the name,
            // each window, and — where there is local spend to describe —
            // what kind of figure it is, have to live in the accessible name
            // or they are lost.
            aria-label={`${limits.displayName}${
              spend ? `, ${usageStateLabel(spend.state).toLocaleLowerCase()}` : ""
            }${windows
              .map(
                (window) =>
                  `, ${liveWindowLabel(window).toLocaleLowerCase()} ${liveWindowValueLabel(
                    window,
                  ).toLocaleLowerCase()}`,
              )
              .join("")}${stale ? `, ${stale.toLocaleLowerCase()}` : ""}`}
            onPointerEnter={(event) => {
              // Touch reports a pointer enter immediately before the click it
              // is about to fire; opening on it would make the tap a toggle
              // that opens and then closes.
              if (event.pointerType === "touch") return
              hoverOpen(limits.provider)
            }}
            onPointerLeave={(event) => {
              if (event.pointerType === "touch") return
              hoverClose()
            }}
            onClick={(event) => {
              cancelHover()
              if (openProvider === limits.provider && openedBy === "pointer") {
                close()
                return
              }
              invokerRef.current = event.currentTarget
              // Flushed synchronously so the panel is in the DOM by the next
              // line: covers both a fresh open and a hover-open promoted to
              // pointer. Deliberately not an effect keyed on [open, openedBy]
              // — `open` is a freshly `.find()`-derived object each render,
              // so its identity changes on every render (including the ones
              // an IPC poll tick causes while the panel just sits open), and
              // an effect on it would re-steal focus each time.
              flushSync(() => {
                setOpenedBy("pointer")
                setOpenProvider(limits.provider)
              })
              const target =
                panelRef.current?.querySelector<HTMLElement>(FOCUSABLE) ?? panelRef.current
              target?.focus()
            }}
            className={cn(
              "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-control px-1.5 type-caption tabular-nums leading-none text-label-secondary hover:bg-surface-hover",
              isOpen && "bg-surface-hover",
            )}
          >
            {maxPercent != null ? (
              // The ring carries the provider's initial rather than replacing
              // it: a chip that says how full something is without saying
              // whose is a worse trade than the ring is worth.
              <UsageRing
                percent={maxPercent}
                mark={providerMark(limits.provider)}
                glyph={providerInitial(limits.displayName)}
                size={22}
                className="shrink-0 text-label-secondary"
              />
            ) : (
              <ProviderGlyph
                displayName={limits.displayName}
                provider={limits.provider}
                size={18}
              />
            )}
            <TextRoll text={value} />
          </button>
        )
      })}

      {limited.length === 0 && (
        <span className="type-caption text-label-tertiary">No live limits</span>
      )}

      {overflow > 0 && (
        <button
          type="button"
          onClick={viewAll}
          aria-label={`Show ${overflow} more provider${overflow === 1 ? "" : "s"}`}
          className="inline-flex h-7 shrink-0 items-center rounded-control px-1 type-caption text-label-tertiary hover:bg-surface-hover"
        >
          +{overflow}
        </button>
      )}

      {open && (
        <div
          ref={panelRef}
          id={panelId}
          role="dialog"
          // Modal only when the reader opened it on purpose. A hover panel
          // that claimed to be modal would be promising containment that is
          // deliberately not there.
          aria-modal={openedBy === "pointer" ? "true" : undefined}
          aria-labelledby={headingId}
          // Keeps the panel alive while the pointer travels into it, and
          // starts the close when it leaves — so the panel is as hoverable as
          // the chip that opened it.
          onPointerEnter={cancelHover}
          onPointerLeave={(event) => {
            if (event.pointerType === "touch") return
            hoverClose()
          }}
          // Focusable so the dialog itself can hold focus when it contains no
          // control; never in the Tab order.
          tabIndex={-1}
          className={cn(
            "ui-anchored-panel absolute left-0 right-0 p-3 outline-none",
            panelAnchor === "up" ? "bottom-full mb-1.5" : "top-full mt-1.5",
          )}
        >
          <ProviderUsageDetail
            displayName={open.displayName}
            providerId={open.provider}
            provider={spendFor(open.provider)}
            live={open}
            now={at}
            headingId={headingId}
            onViewAll={viewAll}
          />
        </div>
      )}
    </div>
  )
}
