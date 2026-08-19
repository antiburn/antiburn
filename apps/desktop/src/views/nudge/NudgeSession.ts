// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { flushSync } from 'react-dom';

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
} from '../../lib/ipc';

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

export type NudgeSnapshot = {
  nudge: Nudge | null;
  entering: boolean;
  exiting: boolean;
  // Collapsed by default (icon + title + reason); hovering expands to reveal the
  // recommendations and the action bar, like a native macOS notification.
  expanded: boolean;
  elapsedLabel: string;
};

const INITIAL_SNAPSHOT: NudgeSnapshot = {
  nudge: null,
  entering: false,
  exiting: false,
  expanded: false,
  elapsedLabel: 'now',
};

/**
 * The imperative boundary between the notification window and the shell.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; IPC
 * subscriptions, timers, and the window-measurement handshake with the nudge
 * crate stay here, where they belong to the external systems that created them
 * rather than to a component lifecycle. See `OnboardingSession` for the same
 * shape applied to a different window.
 *
 * Most updates go through `update`, which notifies listeners on the normal
 * React schedule. A nudge being shown/replaced, or `expanded` toggling, also
 * changes what the card needs to measure — those go through
 * `commitLayoutChange` instead, which forces the notify + re-render to happen
 * synchronously (via `flushSync`) so the component's registered `measure`
 * callback can read the *just-committed* DOM before this task ends. That is
 * exactly the `useLayoutEffect` timing the old measurement effect relied on,
 * reproduced without an effect: measure, then reveal/resize before the browser
 * paints.
 */
export class NudgeSession {
  private listeners = new Set<() => void>();
  private started = false;
  private generation = 0;
  private stopNudgeShow: (() => void) | null = null;
  private stopNudgeHover: (() => void) | null = null;

  // Which nudge id has already been revealed (shown). The first measurement for
  // a nudge sizes + shows the window instantly; later measurements (hover
  // expand / collapse) animate the already-visible window instead.
  private shownId: string | null = null;
  // Wall-clock time the current nudge was set (not when the window is later
  // measured/revealed), so the timestamp label reflects how long it's actually
  // been on screen — it can sit far past its nominal timeout while hovered,
  // since hover pauses auto-dismiss indefinitely.
  private shownAt = 0;
  // The nudge whose entrance animation is still allowed to recover. Clearing
  // this synchronously on animation completion / exit makes a stale watchdog
  // harmless even before its timer callback runs.
  private enteringId: string | null = null;
  // The real dismiss / CTA to run once the exit animation finishes.
  private pendingExit: (() => void) | null = null;

  // Auto-dismiss timer with pause-on-hover. `remaining` lets us resume with the
  // leftover time after the pointer leaves.
  private timerId: number | null = null;
  private deadline = 0;
  private remaining = 0;
  // Whether the pointer is currently over the notification. Tracked
  // independently of `expanded` so a nudge that replaces another under a
  // stationary cursor still knows it's being hovered — otherwise a
  // `mouseenter` never re-fires and the replacement would collapse and
  // auto-dismiss.
  private hovering = false;

  // WebKit can suspend an entrance animation while this prewarmed webview is
  // hidden. Content visibility never depends on that animation (the keyframes
  // are transform-only), and this watchdog also removes a stalled animation's
  // fill state so the card returns to its resting position. A normal
  // `animationend` clears `entering` first and cancels the watchdog.
  private watchdogId: number | null = null;
  // Refresh the timestamp label periodically while a nudge is on screen. A
  // hovered nudge pauses auto-dismiss indefinitely, so it can sit well past its
  // nominal timeout — a static "now" would go stale.
  private elapsedIntervalId: number | null = null;

  private snapshot: NudgeSnapshot = INITIAL_SNAPSHOT;

  // Registered by the component (see `NudgeView`'s `cardRef`) to read the
  // rendered card's height and hand it to the crate. Invoked synchronously
  // after every layout-affecting commit.
  private measure: (() => void) | null = null;

  getSnapshot = (): NudgeSnapshot => this.snapshot;

  /** The component's measurement source, re-registered as the card mounts/unmounts. */
  registerMeasure = (measure: (() => void) | null): void => {
    this.measure = measure;
  };

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    if (!this.started) void this.start();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.stop();
    };
  };

  /**
   * The single hover edge handler. Hovering pauses the auto-dismiss timer and
   * expands the card (progressive disclosure); leaving resumes and collapses it.
   *
   * Two sources report the same edge and either may arrive first, so this
   * de-duplicates on `hovering`:
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
  applyHover = (hovered: boolean, source: 'pointer' | 'native'): void => {
    const changed = this.hovering !== hovered;
    if (changed) {
      this.hovering = hovered;
      if (hovered) {
        this.pause();
      } else {
        this.resume();
      }
      this.commitLayoutChange({ expanded: hovered });
    }
    if (hovered ? source === 'pointer' : changed) {
      void setNudgeHovered(hovered).catch(() => {});
    }
  };

  // Exit guards below: once an exit is armed, a repeat click during the
  // slide-out must not clobber the deferred action or run it twice.
  // `pendingExit` is checked alongside `exiting` because it flips synchronously
  // in `beginExit`, covering a second click in the same tick that the
  // not-yet-rendered `exiting` snapshot would miss.
  handleAction = (action: NudgeAction): void => {
    const nudge = this.snapshot.nudge;
    if (!nudge || this.snapshot.exiting || this.pendingExit) return;
    this.beginExit(() => {
      void nudgeAction(nudge.kind, action.id, action.target).catch(() => {});
    });
  };

  handleClose = (): void => {
    const nudge = this.snapshot.nudge;
    if (!nudge || this.snapshot.exiting || this.pendingExit) return;
    this.beginExit(() => {
      void dismissNudge().catch(() => {});
    });
  };

  /**
   * Clicking the notification body runs its default CTA, like a native macOS
   * notification. `bodyClickAction` picks it, and returns null when the nudge
   * has nothing to run, in which case we send the synthetic `open_app` id,
   * whose only effect is to surface the app.
   */
  handleBodyClick = (): void => {
    const nudge = this.snapshot.nudge;
    // `exiting`/`pendingExit` mean a CTA/close/timeout already armed an exit; a
    // second click on the (now much larger) hit area must not clobber it.
    if (!nudge || this.snapshot.exiting || this.pendingExit) return;
    const chosen = bodyClickAction(nudge.actions ?? []);
    const actionId = chosen?.id ?? NUDGE_OPEN_APP_ACTION_ID;
    this.beginExit(() => {
      void nudgeAction(nudge.kind, actionId, chosen?.target).catch(() => {});
    });
  };

  /**
   * When the *exit* animation finishes, run the deferred dismiss / CTA. (Fires
   * for the enter animation too, but `exiting` is false then, so it's a no-op.)
   *
   * The caller is responsible for the "only the card's own animation, never one
   * bubbled from its content" guard (`event.target !== event.currentTarget`) —
   * that's a DOM concern the component already has the event for.
   */
  handleAnimationEnd = (): void => {
    if (this.snapshot.exiting && this.pendingExit) {
      const run = this.pendingExit;
      this.pendingExit = null;
      run();
      return;
    }
    if (this.snapshot.entering) {
      this.enteringId = null;
      this.clearWatchdog();
      this.update({ entering: false });
    }
  };

  /**
   * The component's measurement callback hands the freshly-measured height back
   * here. The first measurement for a nudge id reveals the (still-hidden)
   * window at its final size; later measurements of the same nudge (hover
   * expand / collapse) resize the already-visible window instead, which on
   * macOS animates the native frame in place like a real notification banner.
   */
  reportMeasuredHeight = (height: number): void => {
    const id = this.snapshot.nudge?.id;
    if (id === undefined) return;
    if (this.shownId !== id) {
      this.shownId = id;
      void revealNudge(height).catch(() => {});
    } else {
      void resizeNudge(height).catch(() => {});
    }
  };

  private start = async (): Promise<void> => {
    this.started = true;
    const generation = ++this.generation;

    // The native hover signal (see `applyHover`).
    const pendingHover = onNudgeHover((hovered) => {
      if (generation !== this.generation) return;
      this.applyHover(hovered, 'native');
    });
    // Subscribe to incoming nudges.
    const pendingShow = onNudgeShow((payload) => {
      if (generation !== this.generation) return;
      this.handleNudgeShow(payload);
    });

    const [stopHover, stopShow] = await Promise.all([pendingHover, pendingShow]);
    if (generation !== this.generation) {
      stopHover();
      stopShow();
      return;
    }
    this.stopNudgeHover = stopHover;
    this.stopNudgeShow = stopShow;
    // Both listeners are attached now — ask the crate to re-deliver any nudge it
    // emitted before we were ready (e.g. right after prewarm). Generation-guarded
    // above so StrictMode's mount/unmount/remount fires this once per real mount.
    void nudgeReady().catch(() => {});
  };

  private stop(): void {
    this.started = false;
    this.generation += 1;
    this.stopNudgeHover?.();
    this.stopNudgeHover = null;
    this.stopNudgeShow?.();
    this.stopNudgeShow = null;
    this.clearTimer();
    this.clearWatchdog();
    this.clearElapsedInterval();
  }

  private handleNudgeShow = (payload: Nudge): void => {
    // Ignore a re-delivery of the nudge already on screen — the crate re-emits
    // the pending payload on `nudgeReady` (above) to cover the case where it
    // was emitted before this listener attached; once shown, skip the repeat so
    // we don't reset the timer.
    if (payload.id === this.shownId) return;

    // A new nudge is replacing the current one. If we were mid-exit with a
    // deferred CTA/dismiss, run it now — the card is about to re-key, so its
    // exit animation would never complete and the pending action would be lost.
    if (this.pendingExit) {
      const run = this.pendingExit;
      this.pendingExit = null;
      run();
    }

    this.enteringId = payload.id;
    this.shownAt = Date.now();
    this.armWatchdog(payload.id);
    this.armElapsedInterval();

    this.commitLayoutChange({
      nudge: payload,
      entering: true,
      exiting: false,
      // Keep the notification expanded if the pointer is already resting on it
      // (a replacement arrived under a stationary cursor); otherwise start
      // collapsed.
      expanded: this.hovering,
      elapsedLabel: 'now',
    });

    if (typeof payload.timeoutMs === 'number') {
      if (this.hovering) {
        // Hold the auto-dismiss paused while hovered; it resumes on mouse-leave.
        this.clearTimer();
        this.remaining = payload.timeoutMs;
      } else {
        this.startTimer(payload.timeoutMs);
      }
    } else {
      // Sticky nudge (no timeout): cancel any timer and clear the leftover so a
      // later hover→leave `resume()` can't arm a timer from a previous nudge.
      this.clearTimer();
      this.remaining = 0;
    }
  };

  // Play the exit animation, then run `action` (see `handleAnimationEnd`).
  private beginExit(action: () => void): void {
    this.clearTimer();
    // Zero the leftover time so a mouse-leave during the exit animation can't
    // `resume()` the dead nudge's auto-dismiss timer and dismiss it a second
    // time. A replacement nudge re-seeds this in `handleNudgeShow`.
    this.remaining = 0;
    this.enteringId = null;
    this.clearWatchdog();
    // Clear the logical hover flag (not the visible `expanded` state — the
    // exiting card keeps rendering as-is while it slides out) so that if a new
    // nudge replaces this one under a stationary cursor, it starts collapsed
    // instead of inheriting the outgoing nudge's expanded state.
    this.hovering = false;
    this.pendingExit = action;
    this.update({ entering: false, exiting: true });
  }

  private startTimer(ms: number): void {
    this.clearTimer();
    if (ms <= 0) return;
    this.remaining = ms;
    this.deadline = Date.now() + ms;
    this.timerId = window.setTimeout(() => {
      this.timerId = null;
      this.beginExit(() => {
        void dismissNudge().catch(() => {});
      });
    }, ms);
  }

  private clearTimer(): void {
    if (this.timerId !== null) {
      window.clearTimeout(this.timerId);
      this.timerId = null;
    }
  }

  private pause(): void {
    if (this.timerId === null) return;
    this.clearTimer();
    this.remaining = Math.max(0, this.deadline - Date.now());
  }

  private resume(): void {
    if (this.remaining > 0) this.startTimer(this.remaining);
  }

  private armWatchdog(nudgeId: string): void {
    this.clearWatchdog();
    this.watchdogId = window.setTimeout(() => {
      this.watchdogId = null;
      if (this.enteringId !== nudgeId || this.snapshot.nudge?.id !== nudgeId) return;
      this.enteringId = null;
      this.update({ entering: false });
    }, ENTER_ANIMATION_FAILSAFE_MS);
  }

  private clearWatchdog(): void {
    if (this.watchdogId !== null) {
      window.clearTimeout(this.watchdogId);
      this.watchdogId = null;
    }
  }

  private armElapsedInterval(): void {
    this.clearElapsedInterval();
    this.elapsedIntervalId = window.setInterval(() => {
      this.update({ elapsedLabel: formatElapsedLabel(Date.now() - this.shownAt) });
    }, 30_000);
  }

  private clearElapsedInterval(): void {
    if (this.elapsedIntervalId !== null) {
      window.clearInterval(this.elapsedIntervalId);
      this.elapsedIntervalId = null;
    }
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }

  /** Non-layout updates (elapsed tick, entering/exiting flips): the normal notify path. */
  private update(change: Partial<NudgeSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    this.notify();
  }

  /**
   * Layout-affecting updates (a nudge shown/replaced, `expanded` toggled): force
   * the notify + re-render to commit synchronously, then measure. `flushSync`
   * is only ever called here, from an IPC callback / timer / DOM handler, never
   * during React's own render or commit — it throws if it is.
   */
  private commitLayoutChange(change: Partial<NudgeSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    flushSync(() => this.notify());
    this.measure?.();
  }
}
