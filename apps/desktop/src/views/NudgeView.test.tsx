// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { Nudge } from '../lib/ipc';
import { NudgeView } from './NudgeView';

/**
 * The notification window.
 *
 * Everything here is state a reader never sees directly: whether the
 * auto-dismiss timer is paused, which CTA an incidental click resolves to, and
 * whether the shell is told exactly once. The shell is stubbed at the module
 * boundary, as everywhere else in this app.
 */

const invoke = vi.hoisted(() => vi.fn());
/** Shell event handlers the view subscribed to, by event name. */
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return () => listeners.delete(name);
  }),
}));

const usageAnomaly: Nudge = {
  id: 'usage-anomaly-1',
  kind: 'usageAnomaly',
  tone: 'warning',
  title: 'Your usage is rising fast',
  reason: 'Well above your usual pace for this session.',
  recommendations: ['Review recent usage', 'Check the provider dashboard'],
  actions: [
    { id: 'dismiss', label: 'Dismiss', primary: false },
    { id: 'view_session', label: 'View session', primary: true },
  ],
  timeoutMs: 10_000,
};

/**
 * A payload with no `recommendations` at all: the wire contract omits an empty
 * array (Rust `skip_serializing_if`), so it arrives with the field absent rather
 * than as `[]`.
 */
const scanFailureNoRecs = {
  id: 'scan-failure-1',
  kind: 'scanFailure',
  tone: 'warning',
  title: 'A scan could not finish',
  reason: 'The last pass stopped before it reached every agent.',
  actions: [
    { id: 'dismiss', label: 'Not now', primary: false },
    { id: 'open_settings', label: 'Open', primary: true },
  ],
  timeoutMs: 10_000,
} as unknown as Nudge;

/** The only primary CTA is a dismiss, so a body click has nothing to run. */
const diskSpaceLow: Nudge = {
  id: 'disk-space-low-1',
  kind: 'diskSpaceLow',
  tone: 'warning',
  title: 'Free space is running low',
  reason: '4 GB left on the startup volume.',
  actions: [{ id: 'dismiss', label: 'Got it', primary: true }],
  timeoutMs: 10_000,
};

/** The settings preview emits no CTAs at all. */
const testNudge: Nudge = {
  id: 'test-1',
  kind: 'test',
  tone: 'info',
  title: 'Notifications are working',
  reason: 'This is a sample notification from your settings.',
  actions: [],
  timeoutMs: 10_000,
};

/** A CTA carrying the session it is about, echoed back to the shell on click. */
const targeted: Nudge = {
  ...usageAnomaly,
  id: 'usage-anomaly-targeted',
  actions: [
    { id: 'dismiss', label: 'Dismiss', primary: false },
    {
      id: 'view_session',
      label: 'View session',
      primary: true,
      target: { type: 'session', agent: 'claude', sessionId: 'session-9', environment: null },
    },
  ],
};

/** Push a nudge at the view, the way the crate's `nudge:show` event does. */
function showNudge(payload: Nudge) {
  act(() => listeners.get('nudge:show')?.({ payload }));
}

/** The crate's native cursor sample (macOS), which bypasses pointer events. */
function nativeHover(hovered: boolean) {
  act(() => listeners.get('nudge:hover')?.({ payload: hovered }));
}

const callsTo = (command: string) =>
  invoke.mock.calls.filter((call) => call[0] === command).length;

/** The `hovered` argument of every `nudge_set_hovered` call, in order. */
const hoverCommandArgs = () =>
  invoke.mock.calls
    .filter((call) => call[0] === 'nudge_set_hovered')
    .map((call) => (call[1] as { hovered: boolean }).hovered);

function notificationCard(container: HTMLElement): HTMLElement {
  const wrapper = container.firstElementChild as HTMLElement;
  return wrapper.firstElementChild as HTMLElement;
}

function finishAnimation(element: Element) {
  fireEvent.animationEnd(element);
  // jsdom lacks style.animation, so React registers its vendor-prefixed
  // fallback listener in tests. Real browsers use the unprefixed event.
  fireEvent(element, new Event('webkitAnimationEnd', { bubbles: true }));
}

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(null);
  listeners.clear();
});

describe('NudgeView', () => {
  it('keeps content rendered while the entrance animation is pending', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);

    expect(notificationCard(container).className).toContain('animate-nudge-in');
    expect(screen.getByText(usageAnomaly.title)).toBeInTheDocument();
    expect(screen.getByText(usageAnomaly.reason!)).toBeInTheDocument();
  });

  it('settles a completed entrance and cancels its watchdog', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<NudgeView />);
      showNudge(usageAnomaly);
      const card = notificationCard(container);
      const timerCountWhileEntering = vi.getTimerCount();

      act(() => {
        finishAnimation(card);
      });

      expect(card.className).not.toContain('animate-nudge-in');
      expect(vi.getTimerCount()).toBe(timerCountWhileEntering - 1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('recovers a stalled entrance after 350ms', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<NudgeView />);
      showNudge(usageAnomaly);
      const card = notificationCard(container);

      act(() => {
        vi.advanceTimersByTime(350);
      });

      expect(card.className).not.toContain('animate-nudge-in');
    } finally {
      vi.useRealTimers();
    }
  });

  it('restarts the entrance watchdog when a nudge is replaced', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<NudgeView />);
      showNudge(usageAnomaly);

      act(() => {
        vi.advanceTimersByTime(300);
      });
      showNudge(scanFailureNoRecs);

      act(() => {
        vi.advanceTimersByTime(50);
      });
      expect(notificationCard(container).className).toContain('animate-nudge-in');

      act(() => {
        vi.advanceTimersByTime(300);
      });
      expect(notificationCard(container).className).not.toContain('animate-nudge-in');
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores animation events bubbled from notification content', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);
    const card = notificationCard(container);

    act(() => {
      finishAnimation(screen.getByText(usageAnomaly.title));
    });

    expect(card.className).toContain('animate-nudge-in');
  });

  it('renders collapsed: title + reason, but hides recommendations and actions', () => {
    render(<NudgeView />);
    showNudge(usageAnomaly);

    expect(screen.getByText(usageAnomaly.title)).toBeInTheDocument();
    expect(screen.getByText(usageAnomaly.reason!)).toBeInTheDocument();
    // Collapsed by default — the expanded detail is not in the DOM yet.
    expect(screen.queryByText('Review recent usage')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'View session' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Dismiss' })).not.toBeInTheDocument();
  });

  it('expands on hover to reveal recommendations + the action bar, and collapses on leave', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);
    const wrapper = container.firstElementChild as HTMLElement;

    act(() => {
      fireEvent.mouseEnter(wrapper);
    });
    expect(screen.getByText('Review recent usage')).toBeInTheDocument();
    expect(screen.getByText('Check the provider dashboard')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'View session' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeInTheDocument();

    act(() => {
      fireEvent.mouseLeave(wrapper);
    });
    expect(screen.queryByText('Review recent usage')).not.toBeInTheDocument();
  });

  it('expands a nudge that has no recommendations field without crashing', () => {
    const { container } = render(<NudgeView />);
    showNudge(scanFailureNoRecs);

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement);
    });

    expect(screen.getByText('A scan could not finish')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Open' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Not now' })).toBeInTheDocument();
  });

  it('reveals on show, then animates a resize on expand so the notification grows in place', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);
    // Sized + shown once on first measurement; no resize yet.
    expect(callsTo('nudge_reveal')).toBe(1);
    expect(callsTo('nudge_resize')).toBe(0);

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement);
    });

    // Expanding remeasures the same nudge → resize (animated), not another reveal.
    expect(callsTo('nudge_reveal')).toBe(1);
    expect(callsTo('nudge_resize')).toBeGreaterThanOrEqual(1);
  });

  it('reveals once, ignoring a re-delivered event with the same id', () => {
    render(<NudgeView />);
    showNudge(usageAnomaly);
    // Re-delivery of the same id: the crate re-emits its pending payload when
    // the webview reports that its listener is attached.
    showNudge(usageAnomaly);

    expect(callsTo('nudge_reveal')).toBe(1);
  });

  it('asks the crate to re-deliver anything emitted before the listener attached', async () => {
    render(<NudgeView />);

    // Resolved on the microtask after `listen` settles, so let it run.
    await act(async () => {});

    expect(callsTo('nudge_ready')).toBe(1);
  });

  it('collapses for the next nudge after an action click, even if the pointer never left', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement);
    });
    expect(screen.getByRole('button', { name: 'View session' })).toBeInTheDocument();

    act(() => {
      fireEvent.click(screen.getByRole('button', { name: 'View session' }));
    });

    // A new nudge replaces it under the still-resting cursor — no `mouseLeave`
    // ever fires — so it should start collapsed rather than inherit the
    // outgoing nudge's expanded state.
    showNudge(scanFailureNoRecs);

    expect(screen.getByText('A scan could not finish')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Open' })).not.toBeInTheDocument();
  });

  it('sends a CTA target through to the shell', () => {
    const { container } = render(<NudgeView />);
    showNudge(targeted);

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement);
    });
    act(() => {
      fireEvent.click(screen.getByRole('button', { name: 'View session' }));
    });
    showNudge(scanFailureNoRecs);

    expect(invoke).toHaveBeenCalledWith('nudge_action', {
      kind: 'usageAnomaly',
      actionId: 'view_session',
      target: { type: 'session', agent: 'claude', sessionId: 'session-9', environment: null },
    });
  });

  it('runs the primary CTA when the notification body is clicked', () => {
    render(<NudgeView />);
    showNudge(usageAnomaly);

    act(() => {
      fireEvent.click(screen.getByText(usageAnomaly.title));
    });
    showNudge(scanFailureNoRecs);

    expect(invoke).toHaveBeenCalledWith('nudge_action', {
      kind: 'usageAnomaly',
      actionId: 'view_session',
      target: undefined,
    });
  });

  it('clicks through the body even while collapsed — no hover needed', () => {
    render(<NudgeView />);
    showNudge(scanFailureNoRecs);

    // No `mouseEnter`: the action bar isn't even rendered, yet the body still
    // resolves the nudge's primary CTA.
    expect(screen.queryByRole('button', { name: 'Open' })).not.toBeInTheDocument();
    act(() => {
      fireEvent.click(screen.getByText('A scan could not finish'));
    });
    showNudge(usageAnomaly);

    expect(invoke).toHaveBeenCalledWith('nudge_action', {
      kind: 'scanFailure',
      actionId: 'open_settings',
      target: undefined,
    });
  });

  it('just opens the app when the body is clicked on a nudge with no actions', () => {
    render(<NudgeView />);
    showNudge(testNudge);

    act(() => {
      fireEvent.click(screen.getByText(testNudge.title));
    });
    showNudge(usageAnomaly);

    expect(invoke).toHaveBeenCalledWith('nudge_action', {
      kind: 'test',
      actionId: 'open_app',
      target: undefined,
    });
  });

  it('just opens the app when the body is clicked and the only primary CTA is a dismiss', () => {
    render(<NudgeView />);
    showNudge(diskSpaceLow);

    act(() => {
      fireEvent.click(screen.getByText(diskSpaceLow.title));
    });
    showNudge(usageAnomaly);

    expect(invoke).toHaveBeenCalledWith('nudge_action', {
      kind: 'diskSpaceLow',
      actionId: 'open_app',
      target: undefined,
    });
  });

  it('does not double-fire when a CTA button is clicked', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement);
    });
    act(() => {
      // A second click during the exit animation must not arm a second action.
      fireEvent.click(screen.getByRole('button', { name: 'View session' }));
      fireEvent.click(screen.getByRole('button', { name: 'View session' }));
    });
    showNudge(scanFailureNoRecs);

    // The action bar is a sibling of the clickable body, so the button's click
    // never bubbles into it either.
    expect(callsTo('nudge_action')).toBe(1);
    expect(invoke).toHaveBeenCalledWith('nudge_action', {
      kind: 'usageAnomaly',
      actionId: 'view_session',
      target: undefined,
    });
  });

  it('does not run an action when the close button is clicked', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement);
    });
    act(() => {
      // A rapid double-click must not dismiss twice.
      fireEvent.click(screen.getByRole('button', { name: 'Close' }));
      fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    });
    showNudge(scanFailureNoRecs);

    expect(callsTo('nudge_dismiss')).toBe(1);
    expect(callsTo('nudge_action')).toBe(0);
  });

  it('dismisses itself when the auto-dismiss timer elapses', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<NudgeView />);
      showNudge(usageAnomaly);

      act(() => {
        vi.advanceTimersByTime(usageAnomaly.timeoutMs!);
      });
      act(() => {
        finishAnimation(notificationCard(container));
      });

      expect(callsTo('nudge_dismiss')).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not re-arm the auto-dismiss timer when the mouse leaves after an exit is armed', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<NudgeView />);
      showNudge(usageAnomaly);
      const wrapper = container.firstElementChild as HTMLElement;

      // Hover pauses the timer with leftover time, close arms the exit, and the
      // subsequent mouse-leave must not resume() the dead nudge's timer.
      act(() => {
        fireEvent.mouseEnter(wrapper);
      });
      act(() => {
        fireEvent.click(screen.getByRole('button', { name: 'Close' }));
      });
      act(() => {
        fireEvent.mouseLeave(wrapper);
        vi.advanceTimersByTime(usageAnomaly.timeoutMs! * 2);
      });
      act(() => {
        finishAnimation(notificationCard(container));
      });

      expect(callsTo('nudge_dismiss')).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not re-arm the auto-dismiss timer when the mouse leaves during a timeout exit', () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<NudgeView />);
      showNudge(usageAnomaly);
      const wrapper = container.firstElementChild as HTMLElement;

      // The timeout fires while the pointer rests on the card, arming the exit.
      // Leaving mid-slide-out must not resume() the dead nudge's timer — that
      // would dismiss the same nudge a second time.
      act(() => {
        fireEvent.mouseEnter(wrapper);
      });
      act(() => {
        fireEvent.mouseLeave(wrapper);
        vi.advanceTimersByTime(usageAnomaly.timeoutMs!);
      });
      act(() => {
        fireEvent.mouseEnter(wrapper);
      });
      act(() => {
        fireEvent.mouseLeave(wrapper);
        vi.advanceTimersByTime(usageAnomaly.timeoutMs! * 2);
      });
      act(() => {
        finishAnimation(notificationCard(container));
      });

      expect(callsTo('nudge_dismiss')).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

/**
 * The native hover signal.
 *
 * The notification never takes macOS key-window status on show, and AppKit
 * routes mouse-moved events to the key window — so while another antiburn
 * window is key, this window gets no pointer events at all. The crate samples
 * the cursor instead and reports crossings as `nudge:hover`; everything hover
 * drives must work from that signal alone.
 */
describe('NudgeView — native hover signal', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    listeners.clear();
  });

  it('expands and pauses auto-dismiss with no pointer events at all', () => {
    vi.useFakeTimers();
    try {
      render(<NudgeView />);
      showNudge(usageAnomaly);

      nativeHover(true);
      expect(screen.getByText('Review recent usage')).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'View session' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Close' }).className).toContain('opacity-100');

      // Auto-dismiss is paused for as long as the cursor rests on it.
      act(() => {
        vi.advanceTimersByTime(usageAnomaly.timeoutMs! * 3);
      });
      expect(callsTo('nudge_dismiss')).toBe(0);

      nativeHover(false);
      expect(screen.queryByText('Review recent usage')).not.toBeInTheDocument();

      act(() => {
        vi.advanceTimersByTime(usageAnomaly.timeoutMs!);
      });
      expect(callsTo('nudge_dismiss')).toBe(0);
      // The dismiss is deferred to the end of the exit animation.
      expect(document.querySelector('.animate-nudge-out')).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not ask for key-window status, but still releases it on leave', () => {
    render(<NudgeView />);
    showNudge(usageAnomaly);

    nativeHover(true);
    // Acquiring key here would take it from the window the user is actually
    // working in — and buys nothing, since the pointer events CSS `:hover`
    // needs are exactly what isn't arriving.
    expect(hoverCommandArgs()).toEqual([]);

    nativeHover(false);
    expect(hoverCommandArgs()).toEqual([false]);
  });

  it('de-duplicates against the pointer, whichever edge lands first', () => {
    const { container } = render(<NudgeView />);
    showNudge(usageAnomaly);
    const wrapper = container.firstElementChild as HTMLElement;

    // Native sample first, pointer second: the pointer enter still requests key
    // (it proves mouse events are flowing) but must not re-expand or re-pause
    // anything.
    nativeHover(true);
    act(() => {
      fireEvent.mouseEnter(wrapper);
    });
    expect(hoverCommandArgs()).toEqual([true]);
    expect(screen.getByRole('button', { name: 'View session' })).toBeInTheDocument();

    // Pointer leave collapses once; the native sample catching up is a no-op.
    act(() => {
      fireEvent.mouseLeave(wrapper);
    });
    nativeHover(false);
    expect(hoverCommandArgs()).toEqual([true, false]);
    expect(screen.queryByText('Review recent usage')).not.toBeInTheDocument();
  });
});
