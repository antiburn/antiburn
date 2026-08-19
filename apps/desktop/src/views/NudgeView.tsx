// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { X } from "lucide-react"
import { useCallback, useState, useSyncExternalStore, type AnimationEvent } from "react"

import appIcon from "../assets/app-icon.png"
import { NudgeSession } from "./nudge/NudgeSession"

/**
 * The notification surface, styled to read like a native macOS notification
 * (HIG): the app icon on the left (identity — not tinted by severity, per HIG),
 * a bold title with a timestamp that swaps to a hover-revealed close, a
 * secondary body line, and the actionable extension (recommendations + CTAs)
 * below. Theme tokens make it adapt to light/dark.
 *
 * Rendered only in the window the nudge crate opens at `#/nudge` (see
 * `lib/route`). On a new nudge it slides in; on dismiss/CTA/timeout it slides
 * out (then the window is hidden). The crate keeps the window hidden until the
 * content is measured and sized, so it never resizes on screen.
 *
 * The component only renders an immutable snapshot and routes DOM events to
 * `NudgeSession`, the external-system boundary that owns IPC subscriptions,
 * timers, and the crate's measure/reveal/resize handshake.
 */
export function NudgeView() {
  const [session] = useState(() => new NudgeSession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )

  // Registers the session's measurement source as the card mounts/unmounts.
  // This has to happen from inside a ref callback rather than the render body:
  // `session.registerMeasure` is a method call (the session owns storing it,
  // not a property assignment on the `useState` value, which the hooks lint
  // rule flags as illegal render-body mutation), but the lint rule *also*
  // flags handing it a closure that reads a ref while called directly from
  // render — it can't know the session won't invoke it synchronously. A ref
  // callback sidesteps both: it only runs during commit, and by the time the
  // session actually calls `measure()` — always from `commitLayoutChange`,
  // always after that commit's own `flushSync` — this has already registered
  // a closure over the just-attached node.
  const cardRef = useCallback(
    (node: HTMLDivElement | null) => {
      session.registerMeasure(
        node
          ? () => session.reportMeasuredHeight(Math.ceil(node.getBoundingClientRect().height))
          : null,
      )
    },
    [session],
  )

  const { nudge, entering, exiting, expanded, elapsedLabel } = state
  if (!nudge) return null

  // The wire contract omits an empty `recommendations` array (Rust
  // `skip_serializing_if`), so it arrives undefined for nudges without any.
  // Default the optional arrays so the expanded render is null-safe.
  const recommendations = nudge.recommendations ?? []
  const actions = nudge.actions ?? []

  function handleAnimationEnd(event: AnimationEvent<HTMLDivElement>) {
    // Only the card's own entrance or exit animation is allowed to settle this
    // state machine — never one bubbled up from its content.
    if (event.target !== event.currentTarget) return
    session.handleAnimationEnd()
  }

  return (
    <div
      onMouseEnter={() => session.applyHover(true, "pointer")}
      onMouseLeave={() => session.applyHover(false, "pointer")}
    >
      <div
        // Re-key per nudge so the enter animation replays for each one.
        key={nudge.id}
        ref={cardRef}
        // The card fills the window edge to edge and paints its own surface.
        // This build does not enable tauri's `macos-private-api`, so the
        // notification window is opaque and undecorated — the same chrome as the
        // popover — and a rounded card would only reveal the window's square
        // corners behind it.
        //
        // Hover-revealed chrome below is driven by `expanded` (which is exactly
        // the hover state) rather than CSS `:hover`/`group-hover`: the pointer
        // events CSS hover depends on don't reach this window while another
        // antiburn window holds macOS key status — see `NudgeSession.applyHover`.
        className={`relative overflow-hidden bg-surface text-label select-none ${
          exiting ? "animate-nudge-out" : entering ? "animate-nudge-in" : ""
        }`}
        onAnimationEnd={handleAnimationEnd}
      >
        {/* The clickable body. Bound here rather than on the card so the action
            bar and the close button — both siblings, below and above this box —
            keep their clicks to themselves and can't double-fire. */}
        <div className="px-4 py-3.5" onClick={session.handleBodyClick}>
          <div className="flex items-center gap-3">
            {/* The app icon — the native "sender" slot. Vertically centered
                against the title + reason, sized to match the native macOS
                notification icon. */}
            <img
              src={appIcon}
              alt=""
              aria-hidden
              draggable={false}
              className="h-12 w-12 shrink-0 select-none"
            />

            <div className="min-w-0 flex-1">
              <div className="flex items-baseline gap-2">
                <p className="type-headline min-w-0 flex-1 truncate leading-tight text-label">
                  {nudge.title}
                </p>
                {/* Timestamp fades on hover as the card expands and the close appears. */}
                <span
                  className={`type-footnote shrink-0 text-label-tertiary transition-opacity ${
                    expanded ? "opacity-0" : "opacity-100"
                  }`}
                >
                  {elapsedLabel}
                </span>
              </div>
              {nudge.reason && (
                <p
                  className={`type-body mt-0.5 leading-snug text-label-secondary ${
                    expanded ? "" : "line-clamp-2"
                  }`}
                >
                  {nudge.reason}
                </p>
              )}
            </div>
          </div>

          {/* Recommendations are the expanded detail — revealed as the native
              window animates open on hover. Indented (icon w-12 + gap-3) so they
              align under the copy while the icon stays centered on the header. */}
          {expanded && recommendations.length > 0 && (
            <ul className="mt-2 space-y-1 pl-[3.75rem]">
              {recommendations.map((rec, i) => (
                <li key={i} className="type-footnote flex gap-1.5 text-label-secondary">
                  <span aria-hidden className="text-label-tertiary">
                    •
                  </span>
                  <span className="min-w-0">{rec}</span>
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* Full-width segmented action bar — revealed on hover, like the macOS
            notification's expanded state. Ghost buttons: no resting fill
            (transparent over the card), hover only. */}
        {expanded && actions.length > 0 && (
          <div className="flex items-stretch border-t border-separator">
            {actions.map((action, i) => (
              <button
                key={action.id}
                onClick={() => session.handleAction(action)}
                className={`type-body flex-1 py-2.5 text-center transition-colors outline-none hover:bg-surface-hover focus-visible:outline-none ${
                  i > 0 ? "border-l border-separator" : ""
                } ${action.primary ? "font-medium text-accent" : "text-label"}`}
              >
                {action.label}
              </button>
            ))}
          </div>
        )}

        {/* Close (✕) — top-right corner, hover-revealed. Named "Close", not
            "Dismiss", so it stays distinct from the "Dismiss" CTA in the action bar. */}
        <button
          onClick={session.handleClose}
          aria-label="Close"
          className={`absolute top-2 right-2 flex h-5 w-5 items-center justify-center rounded-full bg-surface-secondary text-label-secondary transition-opacity outline-none hover:text-label focus-visible:outline-none ${
            expanded ? "opacity-100" : "opacity-0"
          }`}
        >
          <X size={12} aria-hidden />
        </button>
      </div>
    </div>
  )
}
