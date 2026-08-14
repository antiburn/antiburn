// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { X } from 'lucide-react';
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type AnimationEvent,
} from 'react';

import appIcon from '../assets/app-icon.png';
import {
  dismissNudge,
  nudgeAction,
  nudgeReady,
  onNudgeHover,
  onNudgeShow,
  resizeNudge,
  revealNudge,
  setNudgeHovered,
  type Nudge,
  type NudgeAction,
} from '../lib/ipc';

const ENTER_ANIMATION_FAILSAFE_MS = 350;

/** The one CTA id that closes the notification without surfacing the app. */
const NUDGE_DISMISS_ACTION_ID = 'dismiss';

/**
 * Synthetic CTA id sent when the notification body is clicked but the nudge has
 * no action worth running (see {@link bodyClickAction}). It carries no in-app
 * navigation — it exists only because any navigating id makes the shell's
 * `on_action` callback open the popover, which is the whole intent.
 */
const NUDGE_OPEN_APP_ACTION_ID = 'open_app';

/** CTA ids that complete without surfacing the app. */
const NON_NAVIGATING_ACTION_IDS = new Set<string>([NUDGE_DISMISS_ACTION_ID]);

/**
 * Whether a CTA surfaces the app (as opposed to a pure dismiss like "Dismiss" /
 * "Not now").
 */
function actionNavigates(actionId: string): boolean {
  return !NON_NAVIGATING_ACTION_IDS.has(actionId);
}

/**
 * The CTA a click on the notification body should run: the primary one when it
 * is a safe default, else the first other navigating CTA. `null` means the nudge
 * has nothing worth running (no actions, or only dismiss-like ones), and the
 * body click should just open the app.
 *
 * Every CTA this app can post acts inward — nothing a nudge offers leaves the
 * machine — so "safe" here is exactly "navigates somewhere". A CTA that ever did
 * act outward would have to be excluded from this default explicitly: an
 * incidental click on a notification body must never publish anything.
 */
function bodyClickAction(actions: NudgeAction[]): NudgeAction | null {
  const safe = actions.filter((action) => actionNavigates(action.id));
  return safe.find((action) => action.primary) ?? safe[0] ?? null;
}

/** Native-notification-style relative time: "now" under a minute, then "Nm" / "Nh". */
function formatElapsedLabel(elapsedMs: number): string {
  if (elapsedMs < 60_000) return 'now';
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h`;
}

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
 */
export function NudgeView() {
  const [nudge, setNudge] = useState<Nudge | null>(null);
  const [entering, setEntering] = useState(false);
  const [exiting, setExiting] = useState(false);
  // Collapsed by default (icon + title + reason); hovering expands to reveal the
  // recommendations and the action bar, like a native macOS notification.
  const [expanded, setExpanded] = useState(false);
  const cardRef = useRef<HTMLDivElement | null>(null);
  // Which nudge id has already been revealed (shown). The first measurement for a
  // nudge sizes + shows the window instantly; later measurements (hover expand /
  // collapse) animate the already-visible window instead.
  const shownIdRef = useRef<string | null>(null);
  // Wall-clock time the current nudge was set (not when the window is later
  // measured/revealed), so the timestamp label reflects how long it's
  // actually been on screen — it can sit far past its nominal timeout while
  // hovered, since hover pauses auto-dismiss indefinitely.
  const shownAtRef = useRef<number>(0);
  // The nudge whose entrance animation is still allowed to recover. Clearing
  // this synchronously on animation completion / exit makes stale watchdogs
  // harmless even before React runs their effect cleanup.
  const enteringIdRef = useRef<string | null>(null);
  // Recomputed off `shownAtRef` on an interval (see below) rather than
  // derived from `Date.now()` during render, which React disallows.
  const [elapsedLabel, setElapsedLabel] = useState('now');
  // The real dismiss / CTA to run once the exit animation finishes.
  const pendingExit = useRef<(() => void) | null>(null);
  // Mirrors `nudge` for the auto-dismiss timer below: `startTimer` is a stable
  // `useCallback` (deps: `clearTimer`, `beginExit`) so its timeout closure does
  // not see later `nudge` state — read it from this ref instead.
  const nudgeRef = useRef<Nudge | null>(null);

  // Auto-dismiss timer with pause-on-hover. `remainingRef` lets us resume with
  // the leftover time after the pointer leaves.
  const timerRef = useRef<number | null>(null);
  const deadlineRef = useRef<number>(0);
  const remainingRef = useRef<number>(0);
  // Whether the pointer is currently over the notification. Tracked on a
  // *stable* wrapper (not the re-keyed card) so a nudge that replaces another
  // under a stationary cursor still knows it's being hovered — otherwise
  // `mouseenter` never re-fires and the replacement would collapse and
  // auto-dismiss.
  const hoveringRef = useRef(false);

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  // Play the exit animation, then run `action` (see `handleAnimationEnd`).
  const beginExit = useCallback(
    (action: () => void) => {
      clearTimer();
      // Zero the leftover time so a mouse-leave during the exit animation
      // can't `resume()` the dead nudge's auto-dismiss timer and dismiss it a
      // second time. A replacement nudge re-seeds this in the show handler.
      remainingRef.current = 0;
      enteringIdRef.current = null;
      setEntering(false);
      // Clear the logical hover flag (not the visible `expanded` state — the
      // exiting card keeps rendering as-is while it slides out) so that if a
      // new nudge replaces this one under a stationary cursor, it starts
      // collapsed instead of inheriting the outgoing nudge's expanded state.
      hoveringRef.current = false;
      pendingExit.current = action;
      setExiting(true);
    },
    [clearTimer],
  );

  const startTimer = useCallback(
    (ms: number) => {
      clearTimer();
      if (ms <= 0) return;
      remainingRef.current = ms;
      deadlineRef.current = Date.now() + ms;
      timerRef.current = window.setTimeout(() => {
        beginExit(() => {
          void dismissNudge().catch(() => {});
        });
      }, ms);
    },
    [clearTimer, beginExit],
  );

  const pause = useCallback(() => {
    if (timerRef.current === null) return;
    clearTimer();
    remainingRef.current = Math.max(0, deadlineRef.current - Date.now());
  }, [clearTimer]);

  const resume = useCallback(() => {
    if (remainingRef.current > 0) startTimer(remainingRef.current);
  }, [startTimer]);

  /**
   * The single hover edge handler. Hovering pauses the auto-dismiss timer and
   * expands the card (progressive disclosure); leaving resumes and collapses it.
   *
   * Two sources report the same edge and either may arrive first, so this
   * de-duplicates on `hoveringRef`:
   *
   * - `"pointer"` — the window's own `mouseenter`/`mouseleave`. These stop
   *   firing whenever another antiburn window (the settings window, or the
   *   popover) holds macOS key-window status, because AppKit routes mouse-moved
   *   events to the key window and the notification deliberately never takes key
   *   on show.
   * - `"native"` — the crate sampling the cursor against the notification's own
   *   frame (macOS). Immune to event routing, which is the whole point, so it
   *   covers the case above.
   *
   * Only a pointer enter asks for key-window status: that request exists to get
   * continuous `mouseMoved` delivery for CSS `:hover` (and keyboard access to
   * the CTAs), which is worth nothing when the pointer path is already dead —
   * while taking key from the window the user is actually working in would leave
   * them typing into nothing. It asks even when the native sample beat it to the
   * same edge, so losing that race costs nothing. A leave always releases, so
   * key is never held past a hover whichever source reported it.
   */
  const applyHover = useCallback(
    (hovered: boolean, source: 'pointer' | 'native') => {
      const changed = hoveringRef.current !== hovered;
      if (changed) {
        hoveringRef.current = hovered;
        if (hovered) {
          pause();
        } else {
          resume();
        }
        setExpanded(hovered);
      }
      if (hovered ? source === 'pointer' : changed) {
        void setNudgeHovered(hovered).catch(() => {});
      }
    },
    [pause, resume],
  );

  // The native hover signal (see `applyHover`).
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void onNudgeHover((hovered) => applyHover(hovered, 'native')).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [applyHover]);

  // Subscribe to incoming nudges.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void onNudgeShow((payload) => {
      // Ignore a re-delivery of the nudge already on screen — the crate
      // re-emits the pending payload on `nudgeReady` (below) to cover the case
      // where it was emitted before this listener attached; once shown, skip
      // the repeat so we don't reset the timer.
      if (payload.id === shownIdRef.current) return;

      // A new nudge is replacing the current one. If we were mid-exit with a
      // deferred CTA/dismiss, run it now — the card is about to re-key, so its
      // exit animation would never complete and the pending action would be lost.
      if (pendingExit.current) {
        const run = pendingExit.current;
        pendingExit.current = null;
        run();
      }

      setExiting(false);
      enteringIdRef.current = payload.id;
      setEntering(true);
      // Keep the notification expanded if the pointer is already resting on it
      // (a replacement arrived under a stationary cursor); otherwise start
      // collapsed.
      setExpanded(hoveringRef.current);
      setNudge(payload);
      nudgeRef.current = payload;
      shownAtRef.current = Date.now();
      setElapsedLabel('now');
      if (typeof payload.timeoutMs === 'number') {
        if (hoveringRef.current) {
          // Hold the auto-dismiss paused while hovered; it resumes on mouse-leave.
          clearTimer();
          remainingRef.current = payload.timeoutMs;
        } else {
          startTimer(payload.timeoutMs);
        }
      } else {
        // Sticky nudge (no timeout): cancel any timer and clear the leftover so a
        // later hover→leave `resume()` can't arm a timer from a previous nudge.
        clearTimer();
        remainingRef.current = 0;
      }
    }).then((fn) => {
      unlisten = fn;
      // The listener is attached now — ask the crate to re-deliver any nudge it
      // emitted before we were ready (e.g. right after prewarm).
      void nudgeReady().catch(() => {});
    });
    return () => {
      unlisten?.();
      clearTimer();
    };
  }, [startTimer, clearTimer]);

  // WebKit can suspend an entrance animation while this prewarmed webview is
  // hidden. Content visibility never depends on that animation (the keyframes
  // are transform-only), and this watchdog also removes a stalled animation's
  // fill state so the card returns to its resting position. A normal
  // `animationend` clears `entering` first and cancels the watchdog.
  const enteringNudgeId = nudge?.id;
  useEffect(() => {
    if (!enteringNudgeId || !entering || exiting) return;
    const timer = window.setTimeout(() => {
      if (enteringIdRef.current !== enteringNudgeId || nudgeRef.current?.id !== enteringNudgeId)
        return;
      enteringIdRef.current = null;
      setEntering(false);
    }, ENTER_ANIMATION_FAILSAFE_MS);
    return () => window.clearTimeout(timer);
  }, [entering, enteringNudgeId, exiting]);

  // Measure the rendered content and hand the height to the crate. On a nudge's
  // first measurement we `reveal` — the crate sizes + shows the (still-hidden)
  // window so it appears at its final size with no wobble. On later measurements
  // (hover expand / collapse, same nudge) we `resize` the already-visible window,
  // which on macOS animates the native frame with the system ease so it grows /
  // shrinks in place like a real notification banner. `getBoundingClientRect`
  // forces layout, so the height is correct even while the window is hidden.
  useLayoutEffect(() => {
    if (!nudge || !cardRef.current) return;
    const height = Math.ceil(cardRef.current.getBoundingClientRect().height);
    if (shownIdRef.current !== nudge.id) {
      shownIdRef.current = nudge.id;
      void revealNudge(height).catch(() => {});
    } else {
      void resizeNudge(height).catch(() => {});
    }
  }, [nudge, expanded]);

  // Refresh the timestamp label periodically while a nudge is on screen. A
  // hovered nudge pauses auto-dismiss indefinitely, so it can sit well past
  // its nominal timeout — a static "now" would go stale.
  const shownNudgeId = nudge?.id ?? null;
  useEffect(() => {
    if (!shownNudgeId) return;
    const id = window.setInterval(() => {
      setElapsedLabel(formatElapsedLabel(Date.now() - shownAtRef.current));
    }, 30_000);
    return () => window.clearInterval(id);
  }, [shownNudgeId]);

  // Exit guards below: once an exit is armed, a repeat click during the
  // slide-out must not clobber the deferred action or run it twice.
  // `pendingExit` is checked alongside `exiting` because the ref flips
  // synchronously in `beginExit`, covering a second click in the same React
  // batch that the not-yet-rendered `exiting` state would miss.
  function handleAction(action: NudgeAction) {
    if (!nudge || exiting || pendingExit.current) return;
    beginExit(() => {
      void nudgeAction(nudge.kind, action.id, action.target).catch(() => {});
    });
  }

  function handleClose() {
    if (!nudge || exiting || pendingExit.current) return;
    beginExit(() => {
      void dismissNudge().catch(() => {});
    });
  }

  // When the *exit* animation finishes, run the deferred dismiss / CTA. (Fires
  // for the enter animation too, but `exiting` is false then, so it's a no-op.)
  function handleAnimationEnd(event: AnimationEvent<HTMLDivElement>) {
    // Only the card's own entrance or exit animation is allowed to settle this
    // state machine — never one bubbled up from its content.
    if (event.target !== event.currentTarget) return;
    if (exiting && pendingExit.current) {
      const run = pendingExit.current;
      pendingExit.current = null;
      run();
      return;
    }
    if (entering) {
      enteringIdRef.current = null;
      setEntering(false);
    }
  }

  if (!nudge) return null;

  // The wire contract omits an empty `recommendations` array (Rust
  // `skip_serializing_if`), so it arrives undefined for nudges without any.
  // Default the optional arrays so the expanded render is null-safe.
  const recommendations = nudge.recommendations ?? [];
  const actions = nudge.actions ?? [];

  // Clicking the notification body runs its default CTA, like a native macOS
  // notification. `bodyClickAction` picks it, and returns null when the nudge has
  // nothing to run, in which case we send the synthetic `open_app` id, whose only
  // effect is to surface the app.
  function handleBodyClick() {
    // `exiting`/`pendingExit` mean a CTA/close/timeout already armed an exit; a
    // second click on the (now much larger) hit area must not clobber it.
    if (!nudge || exiting || pendingExit.current) return;
    const chosen = bodyClickAction(actions);
    const actionId = chosen?.id ?? NUDGE_OPEN_APP_ACTION_ID;
    beginExit(() => {
      void nudgeAction(nudge.kind, actionId, chosen?.target).catch(() => {});
    });
  }

  return (
    <div
      onMouseEnter={() => applyHover(true, 'pointer')}
      onMouseLeave={() => applyHover(false, 'pointer')}
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
        // antiburn window holds macOS key status — see `applyHover`.
        className={`relative overflow-hidden bg-surface text-label select-none ${
          exiting ? 'animate-nudge-out' : entering ? 'animate-nudge-in' : ''
        }`}
        onAnimationEnd={handleAnimationEnd}
      >
        {/* The clickable body. Bound here rather than on the card so the action
            bar and the close button — both siblings, below and above this box —
            keep their clicks to themselves and can't double-fire. */}
        <div className="px-4 py-3.5" onClick={handleBodyClick}>
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
                    expanded ? 'opacity-0' : 'opacity-100'
                  }`}
                >
                  {elapsedLabel}
                </span>
              </div>
              {nudge.reason && (
                <p
                  className={`type-body mt-0.5 leading-snug text-label-secondary ${
                    expanded ? '' : 'line-clamp-2'
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
                onClick={() => handleAction(action)}
                className={`type-body flex-1 py-2.5 text-center transition-colors outline-none hover:bg-surface-hover focus-visible:outline-none ${
                  i > 0 ? 'border-l border-separator' : ''
                } ${action.primary ? 'font-medium text-accent' : 'text-label'}`}
              >
                {action.label}
              </button>
            ))}
          </div>
        )}

        {/* Close (✕) — top-right corner, hover-revealed. Named "Close", not
            "Dismiss", so it stays distinct from the "Dismiss" CTA in the action bar. */}
        <button
          onClick={handleClose}
          aria-label="Close"
          className={`absolute top-2 right-2 flex h-5 w-5 items-center justify-center rounded-full bg-surface-secondary text-label-secondary transition-opacity outline-none hover:text-label focus-visible:outline-none ${
            expanded ? 'opacity-100' : 'opacity-0'
          }`}
        >
          <X size={12} aria-hidden />
        </button>
      </div>
    </div>
  );
}
