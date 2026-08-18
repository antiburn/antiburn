// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { LogicalPosition } from '@tauri-apps/api/dpi';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import { X } from 'lucide-react';
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from 'react';
import {
  getLatestSessionActivity,
  getLiveUsage,
  openSettingsWindow,
  type LiveUsageSummaryPayload,
} from '../lib/ipc';
import { setFloatingHudEnabled } from '../lib/overlayWindow';
import { deriveUsageBars, resetsIn } from '../lib/usageBars';
import { LedBar } from '../components/ui/LedBar';

const REFRESH_MS = 60_000;

/** How recently the newest transcript write must have landed for the blink to
 * run. The mtimes come from the shell's discovery memo (60s TTL), so during
 * a live session the reported write can look up to ~60s stale — the window
 * must clear that with margin or the blink flaps mid-session. 90s means the
 * blink dies ~1.5 minutes after the session actually goes quiet. */
const LIVE_WINDOW_SECS = 90;

/** How often the overlay re-checks session liveness. */
const LIVENESS_POLL_MS = 5_000;

/** Breathing room kept between the expanded panel and the edge of the
 * display. */
const SCREEN_MARGIN = 8;

/** How far the panel is inset from the top of the window frame when it needs
 * room to open upward. Kept at zero the rest of the time: macOS refuses to
 * place a window's top edge above the menu bar, so a permanent inset put a
 * floor under the HUD this far below the top of the screen. Switching it is
 * always paired with an equal, opposite move of the window, so the panel
 * stays exactly where it is on screen. */
const RESERVE_ABOVE = 220;

/** Rough height of the expanded panel, used to reserve room on screen before
 * it has ever been drawn: fixed chrome (header, padding) plus a row per limit
 * (label, bar, reset line). Replaced by the real measurement the first time
 * the panel expands. */
const ESTIMATED_CHROME_PX = 48;
const ESTIMATED_ROW_PX = 50;

/** Dots per bar in the HUD. Kept low for a narrow panel — the dots keep
 * their size and spacing, there are just fewer of them. Resolution costs
 * nothing here: nobody reads a percentage off the dots, the number below
 * them says it. */
const HUD_SEGMENTS = 20;

/** Hover-intent dwell: the cursor must sit on the panel this long before it
 * expands, so mousing past on the way somewhere else doesn't trigger it.
 * Collapse stays immediate. */
const HOVER_INTENT_MS = 250;

/**
 * The antiburn overlay: an always-on-top transparent window parked under the
 * menu bar. At rest it draws nothing but the LED usage bars; hovering
 * assembles a panel around them with labels, percentages, and a ✕ that hides
 * the window (the popover's pop-out button re-shows it). The window frame is
 * fixed at the expanded size — expansion is pure CSS inside it — and the
 * panel is draggable anywhere but its two buttons, so once revealed you can
 * pick the whole thing up. Picking it up collapses it again for the duration of the
 * drag, so you can see where you are putting it.
 */
export function OverlayWindow() {
  const [response, setResponse] = useState<LiveUsageSummaryPayload | null>(null);
  const [hovered, setHovered] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [sessionLive, setSessionLive] = useState(false);
  const panelRef = useRef<HTMLDivElement | null>(null);

  // The shared stylesheet paints every window's body opaque; the overlay is
  // the app's one transparent window, so it clears that ground for its own
  // document (see `body[data-transparent-window]` in styles/hud.css).
  useEffect(() => {
    document.body.dataset.transparentWindow = 'true';
    return () => {
      delete document.body.dataset.transparentWindow;
    };
  }, []);

  // Hover-intent gate shared by both hover paths (DOM events while focused,
  // the Rust cursor watcher while backgrounded): entering starts a short
  // dwell timer before expanding; leaving cancels it and collapses at once.
  const hoverTimerRef = useRef<number | null>(null);
  const requestHover = useCallback((next: boolean) => {
    if (next) {
      if (hoverTimerRef.current != null) return;
      hoverTimerRef.current = window.setTimeout(() => {
        hoverTimerRef.current = null;
        setHovered(true);
      }, HOVER_INTENT_MS);
    } else {
      if (hoverTimerRef.current != null) {
        window.clearTimeout(hoverTimerRef.current);
        hoverTimerRef.current = null;
      }
      setHovered(false);
    }
  }, []);
  useEffect(
    () => () => {
      if (hoverTimerRef.current != null) window.clearTimeout(hoverTimerRef.current);
    },
    [],
  );

  // Carrying the HUD collapses it: an expanded panel covers whatever you are
  // trying to park it against. Hover still governs the panel otherwise, so
  // dropping it back under the cursor re-expands it.
  const [dragging, setDragging] = useState(false);
  // Mirror of the flag for the hover-region reporter, which runs from a
  // ResizeObserver and needs the value before effects have flushed.
  const draggingRef = useRef(false);
  const expanded = hovered && !dragging;

  const markDragging = useCallback((next: boolean) => {
    draggingRef.current = next;
    setDragging(next);
  }, []);

  // The drag is done by hand rather than with `data-tauri-drag-region` so the
  // window can be moved and the panel's inset changed in the same breath.
  const dragOriginRef = useRef<{
    pointerX: number;
    pointerY: number;
    windowX: number;
    windowY: number;
  } | null>(null);

  // Space held above the panel inside the frame, and the swap that changes
  // it. Because the window moves by the same amount in the opposite
  // direction, the panel does not shift on screen — but the two happen a
  // frame apart, so the panel is hidden across the swap rather than seen
  // jumping and coming back.
  const [reserveAbove, setReserveAbove] = useState(0);
  const [swapping, setSwapping] = useState(false);
  const reserveRef = useRef(0);

  const applyReserve = useCallback(async (desired: number) => {
    const current = reserveRef.current;
    if (desired === current) return;
    reserveRef.current = desired;
    setSwapping(true);
    try {
      // Let the hidden panel paint before the window is asked to move.
      await new Promise<number>((resolve) => requestAnimationFrame(resolve));
      const overlay = getCurrentWindow();
      const [monitor, position] = await Promise.all([
        currentMonitor(),
        overlay.outerPosition(),
      ]);
      const scale = monitor?.scaleFactor ?? 1;
      await overlay.setPosition(
        new LogicalPosition(position.x / scale, position.y / scale + current - desired),
      );
    } finally {
      setReserveAbove(desired);
      setSwapping(false);
    }
  }, []);

  const startDrag = useCallback(
    (event: ReactMouseEvent) => {
      // The wordmark and ✕ are real buttons; only the bare surface drags.
      if ((event.target as HTMLElement).closest('button')) return;
      const { screenX, screenY } = event;
      void (async () => {
        // Carry the HUD with no space above it, so nothing but the panel
        // itself can meet the menu bar and stop the drag short.
        await applyReserve(0);
        const [monitor, position] = await Promise.all([
          currentMonitor(),
          getCurrentWindow().outerPosition(),
        ]);
        const scale = monitor?.scaleFactor ?? 1;
        dragOriginRef.current = {
          pointerX: screenX,
          pointerY: screenY,
          windowX: position.x / scale,
          windowY: position.y / scale,
        };
        markDragging(true);
      })();
    },
    [applyReserve, markDragging],
  );

  // Release is the only thing that ends a drag. Moves and hover changes keep
  // arriving throughout — treating any of them as the end made the panel flap
  // between states while the window was being carried.
  useEffect(() => {
    if (!dragging) return;
    // One move per frame: the pointer fires faster than the window can be
    // asked to move, and the extra calls only queue up behind the screen.
    let pending: MouseEvent | null = null;
    let frame = 0;
    const apply = () => {
      frame = 0;
      const origin = dragOriginRef.current;
      if (!pending || !origin) return;
      const { screenX, screenY } = pending;
      pending = null;
      void getCurrentWindow().setPosition(
        new LogicalPosition(
          origin.windowX + (screenX - origin.pointerX),
          origin.windowY + (screenY - origin.pointerY),
        ),
      );
    };
    const move = (event: MouseEvent) => {
      pending = event;
      if (!frame) frame = requestAnimationFrame(apply);
    };
    const stop = () => markDragging(false);
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', stop, true);
    window.addEventListener('blur', stop);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', stop, true);
      window.removeEventListener('blur', stop);
    };
  }, [dragging, markDragging]);

  // Liveness signal: the newest transcript-file mtime across all discovered
  // sessions, straight from the shell's discovery memo. The session index's
  // own timestamps freeze while the popover is closed (the scan scheduler
  // pauses), and an `isActive` flag lingers minutes after a stop — real file
  // mtimes are the one signal that moves while a session works and freezes
  // the moment it stops.
  useEffect(() => {
    let cancelled = false;
    const check = () =>
      getLatestSessionActivity()
        .then((latest) => {
          if (cancelled) return;
          setSessionLive(latest != null && Date.now() / 1000 - latest <= LIVE_WINDOW_SECS);
        })
        .catch(() => {});
    void check();
    const id = window.setInterval(check, LIVENESS_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  // Tell the Rust hover watcher how tall the drawn panel actually is, so the
  // hover zone tracks visible pixels instead of the whole (mostly transparent)
  // fixed window frame. Re-reported on every resize — including the CSS
  // expand/collapse.
  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    const report = () => {
      // Hold the last reported region for the length of the drag: the
      // collapse would otherwise shrink the hover zone out from under the
      // cursor and collapse the panel for good once the drag ends.
      if (draggingRef.current) return;
      const rect = panel.getBoundingClientRect();
      invoke('set_overlay_hover_region', {
        top: rect.top,
        bottom: rect.bottom,
      }).catch(() => {});
    };
    report();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(report);
    observer.observe(panel);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    let cancelled = false;

    // Poll only — unlike the source app, this shell emits no push event when
    // a live-usage reading changes, so the 60s refresh is the whole story.
    const refresh = () =>
      getLiveUsage()
        .then((next) => {
          if (!cancelled) setResponse(next);
        })
        .catch(() => {});

    void refresh();
    const poll = window.setInterval(() => void refresh(), REFRESH_MS);
    const clock = window.setInterval(() => setNow(Date.now()), 30_000);

    // Authoritative hover signal from the Rust cursor watcher: macOS doesn't
    // deliver mouse events to a background app's webview, so without this the
    // HUD would only expand once the app was clicked into focus. The DOM
    // handlers below stay as the zero-latency path while focused; this event
    // corrects any missed enter/leave within ~100ms.
    let unlistenHover: (() => void) | null = null;
    void listen<boolean>('overlay_hover', (event) => {
      if (!cancelled) requestHover(Boolean(event.payload));
    })
      .then((dispose) => {
        if (cancelled) dispose();
        else unlistenHover = dispose;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      window.clearInterval(poll);
      window.clearInterval(clock);
      unlistenHover?.();
    };
  }, [requestHover]);

  const bars = deriveUsageBars(response);

  // Which way the panel opens. The HUD never travels anywhere the user did
  // not put it — moving it on expand, on collapse, or on drop all ended badly
  // — so instead a HUD parked too low to open downward is given transparent
  // space above the panel to open upward into.
  const [flipUp, setFlipUp] = useState(false);
  // Both states measured as they are drawn: the collapsed height anchors a
  // flipped panel's bottom edge, the expanded height decides whether to flip
  // at all. An estimate stands in until the HUD has been opened once.
  const [panelHeights, setPanelHeights] = useState({
    collapsed: 0,
    expanded: 0,
  });

  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    const height = panel.getBoundingClientRect().height;
    setPanelHeights((previous) => {
      if (expanded)
        return height > previous.expanded ? { ...previous, expanded: height } : previous;
      return height !== previous.collapsed ? { ...previous, collapsed: height } : previous;
    });
  }, [expanded, bars.length]);

  // Decided while at rest — after a drag, and when a new limit changes the
  // height needed — never mid-hover.
  useEffect(() => {
    if (dragging) return;
    let cancelled = false;
    const decide = async () => {
      const [monitor, position] = await Promise.all([
        currentMonitor(),
        getCurrentWindow().outerPosition(),
      ]);
      if (cancelled || !monitor) return;
      const scale = monitor.scaleFactor;
      const screenBottom = (monitor.position.y + monitor.size.height) / scale - SCREEN_MARGIN;
      const panelTop = position.y / scale + reserveRef.current;
      const needed = Math.max(
        panelHeights.expanded,
        ESTIMATED_CHROME_PX + bars.length * ESTIMATED_ROW_PX,
      );
      const flip = panelTop + needed > screenBottom;
      setFlipUp(flip);
      await applyReserve(flip ? RESERVE_ABOVE : 0);
    };
    void decide();
    return () => {
      cancelled = true;
    };
  }, [applyReserve, dragging, bars.length, panelHeights.expanded]);

  // Flipped, the panel hangs from where its collapsed self sat, so it grows
  // into the space above the bars instead of off the bottom of the screen.
  const lift =
    expanded && flipUp ? Math.max(0, panelHeights.expanded - panelHeights.collapsed) : 0;

  return (
    <div className="h-screen w-screen bg-transparent">
      <div
        ref={panelRef}
        onMouseEnter={() => requestHover(true)}
        onMouseLeave={() => requestHover(false)}
        className={`mx-2 select-none rounded-xl transition-all duration-150 ${
          expanded
            ? 'bevel border border-separator px-3 pt-2 pb-3'
            : 'border border-transparent px-3 pt-2 pb-2'
        }`}
        style={{
          marginTop: reserveAbove - lift,
          backgroundColor: expanded ? 'var(--color-bg-hud)' : 'transparent',
          visibility: swapping ? 'hidden' : 'visible',
        }}
        onMouseDown={startDrag}
      >
        {/* mousedown bubbles up to the container's handler from here too, so
            the header is as draggable as the rest of the panel; the wordmark
            and ✕ buttons opt out inside `startDrag`. */}
        <div
          className={`flex items-center justify-between transition-opacity duration-150 ${
            expanded ? 'opacity-100 mb-1.5' : 'pointer-events-none h-0 opacity-0'
          }`}
        >
          <button
            type="button"
            aria-label="Open antiburn settings"
            onClick={() => openSettingsWindow('general').catch(() => {})}
            className="font-bitcount text-[11px] text-label-tertiary lowercase hover:text-label-secondary transition-colors"
          >
            antiburn
          </button>
          <button
            type="button"
            aria-label="Close overlay"
            onClick={() => {
              // Closing from the HUD flips the Settings preference off too,
              // so the toggle stays truthful.
              setFloatingHudEnabled(false);
              void getCurrentWindow().hide();
            }}
            className="p-1 -m-1 rounded-md text-label-tertiary hover:text-label-secondary hover:bg-surface-tertiary transition-colors"
          >
            <X size={13} />
          </button>
        </div>

        {bars.length === 0 ? (
          expanded ? (
            <p className="type-caption text-label-tertiary pointer-events-none">
              No usage limits detected yet.
            </p>
          ) : (
            <div className="pointer-events-none">
              <LedBar segments={HUD_SEGMENTS} split={[]} />
            </div>
          )
        ) : (
          /* pointer-events-none on the whole bar stack: nothing in it is
             interactive, and it keeps every mousedown addressed to the
             container so the HUD can be dragged by its charts. */
          <div className={`pointer-events-none ${expanded ? 'space-y-2' : 'space-y-[3px]'}`}>
            {bars.map((bar, index) => (
              <div key={bar.key}>
                {expanded && (
                  <div className="flex items-baseline justify-between gap-2 type-caption">
                    <span className="led-caption text-label-secondary truncate">
                      {bar.label}
                    </span>
                    <span className="stats-number text-[13px] text-label shrink-0">
                      {Math.round(bar.percent)}%
                    </span>
                  </div>
                )}
                <LedBar
                  segments={HUD_SEGMENTS}
                  className={expanded ? 'mt-1' : ''}
                  split={[{ fraction: bar.percent / 100, color: bar.color }]}
                  blinkLast={sessionLive && index === 0}
                />
                {expanded && (
                  <p className="led-caption type-footnote text-label-secondary mt-0.5">
                    {resetsIn(bar.resetsAt, now)}
                  </p>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
