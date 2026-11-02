import type {
  EngineInterface,
  RenderElement,
  RenderInput,
  RenderNode,
} from "claude-code";
import type { LiveBurnState, LiveUsageState } from "../types";
import { BRAND_MARK_SVG } from "./register-visuals";
import { renderUsageMeter } from "./register-usage-meter";
import {
  renderCompactUsageBand,
  renderDesktopUsageBand,
  renderEmptyUsageBand,
} from "./register-usage-bands";
import {
  pace,
  percentage,
  primaryWindow,
  terminalDotCount,
  PRIMARY_WINDOWS,
  type UsageSnapshot,
  type UsageWindow,
} from "./usage";
import {
  markSvg,
  meterSvg,
  responsiveMeterSvg,
  usageStatusSvg,
} from "./usage-graphics";
import {
  desktopPercentage,
  meterGeometry,
  type usageLayout,
} from "./usage-presentation";
import { usageFixture, previewRetryText } from "./usage-fixtures";

type AbovePromptEvent = RenderInput<"AbovePrompt">;
type UsageUi = ReturnType<EngineInterface["ui"]["resolve"]>;
type Fixture = NonNullable<ReturnType<typeof usageFixture>>;
type DesktopLayout = ReturnType<typeof usageLayout>;

export const hoverSites = new Map<string, { shape: string; binding: string }>();
const hoverMount = Math.random().toString(36).slice(2);
let hoverRevision = 0;

export function terminalHoverBinding(requestId: string, shape: string): string {
  const previous = hoverSites.get(requestId);
  if (previous?.shape === shape) return previous.binding;
  const binding = `${hoverMount}:${++hoverRevision}`;
  if (hoverSites.size >= 8 && !hoverSites.has(requestId))
    hoverSites.delete(hoverSites.keys().next().value!);
  hoverSites.set(requestId, { shape, binding });
  return binding;
}

export type DesktopAssetCache = {
  key: string | undefined;
  sources: string[];
  responsiveSources?: string[];
  mark: string;
  markEntranceAt: number | null | undefined;
  markReplayRevision: number;
};

export function buildDesktopUsageAssets(input: {
  mount: string;
  snapshot: UsageSnapshot;
  preview?: LiveUsageState["preview"];
  state: LiveUsageState;
  fixture?: Fixture;
  windows: UsageWindow[];
  now: number;
  liveNow: number;
  layout: DesktopLayout;
  variant: "inline" | "current";
  entranceStartedAt: number | null;
  roll: { previous: UsageSnapshot; startedAt: number } | undefined;
  previewFill: { from: number[]; to: number[]; startedAt: number } | undefined;
  replayRevision: number;
  cache: DesktopAssetCache;
}): DesktopAssetCache {
  const {
    mount,
    snapshot,
    preview,
    state,
    fixture,
    windows,
    now,
    liveNow,
    layout,
    variant,
    entranceStartedAt,
    roll,
    previewFill,
    replayRevision,
    cache,
  } = input;
  const key = JSON.stringify([
    mount,
    snapshot,
    preview?.id,
    state.previewReveal,
    replayRevision,
  ]);
  if (cache.key === key) return cache;
  const elapsed =
    entranceStartedAt === null
      ? undefined
      : Math.max(0, liveNow - entranceStartedAt);
  const rollElapsed =
    roll === undefined ? undefined : Math.max(0, liveNow - roll.startedAt);
  const frames = windows.map((window, index) => {
    const geometry = meterGeometry(window, layout.meterWidth, now, variant);
    const motion: Parameters<typeof meterSvg>[5] = {
      previousFillFraction:
        fixture && previewFill
          ? Math.max(
              0,
              Math.min(
                1,
                (previewFill.from[index]! - geometry.trackX) /
                  Math.max(1, geometry.trackWidth),
              ),
            )
          : undefined,
      layout: variant,
      resetPreview: fixture ? state.previewReveal : undefined,
      warning: fixture?.warning,
      previousFillEnd:
        fixture && previewFill ? previewFill.from[index] : undefined,
      fillElapsed:
        fixture && previewFill ? liveNow - previewFill.startedAt : undefined,
      entranceElapsed: elapsed,
      meterIndex: index as 0 | 1,
      previousPercent:
        roll === undefined
          ? undefined
          : desktopPercentage(
              primaryWindow(
                roll.previous,
                window.kind as "five_hour" | "seven_day",
              ),
            ),
      rollElapsed,
    };
    const revealed = fixture !== undefined && state.previewReveal === "show";
    const tag = (source: string) =>
      source.replace("<svg ", `<svg data-entrance="${replayRevision}" `);
    return {
      fixed: tag(
        meterSvg(window, layout.meterWidth, now, revealed, true, motion),
      ),
      responsive: tag(responsiveMeterSvg(window, now, motion, revealed)),
    };
  });
  const sources = frames.map((frame) => frame.fixed);
  const responsiveSources = frames.map((frame) => frame.responsive);
  let mark = cache.mark;
  let markEntranceAt = cache.markEntranceAt;
  let markReplayRevision = cache.markReplayRevision;
  if (
    !mark ||
    (elapsed !== undefined &&
      (markEntranceAt !== entranceStartedAt ||
        markReplayRevision !== replayRevision))
  ) {
    mark = markSvg(BRAND_MARK_SVG, elapsed).replace(
      "<svg ",
      `<svg data-entrance="${replayRevision}" `,
    );
    markEntranceAt = entranceStartedAt;
    markReplayRevision = replayRevision;
  }
  return {
    key,
    sources,
    responsiveSources,
    mark,
    markEntranceAt,
    markReplayRevision,
  };
}

type UsageFrame = {
  ui: UsageUi;
  state: LiveUsageState;
  burnState: LiveBurnState;
  liveNow: number;
};

type DesktopUsageInput = {
  event: AbovePromptEvent;
  state: LiveUsageState;
  snapshot: UsageSnapshot;
  preview?: LiveUsageState["preview"];
  fixture?: Fixture;
  windows: UsageWindow[];
  now: number;
  liveNow: number;
  variant: "inline" | "current";
};

type UsageRuntime = {
  load: (event: AbovePromptEvent) => Promise<UsageFrame>;
  settle: (now: number) => void;
  setVariant: (variant: "inline" | "current") => void;
  prepareBurn: (
    state: LiveBurnState,
    now: number,
  ) => {
    active: boolean;
    revealExpires: number;
  };
  prepareDesktop: (input: DesktopUsageInput) => {
    layout: DesktopLayout;
    sources: string[];
    responsiveSources: string[];
    mark: string;
  };
  empty: (event: AbovePromptEvent) => void;
  cancel: () => void;
  binding: (event: AbovePromptEvent, shape: string) => string;
  pulse: (
    event: AbovePromptEvent,
    isDesktop: boolean,
    preview: LiveUsageState["preview"],
    snapshot: UsageSnapshot,
    windows: UsageWindow[],
    state: LiveBurnState,
    now: number,
    warning: Fixture["warning"] | undefined,
  ) => boolean;
  renderMark: (
    event: AbovePromptEvent,
    source: string,
    alt: string,
  ) => RenderNode | null;
  renderTerminalMark: (
    event: AbovePromptEvent,
    state: LiveBurnState,
    now: number,
    active: boolean,
  ) => RenderNode;
  refreshPreview: (id: string) => void;
  reveal: (kind: string) => Promise<void>;
};

export async function renderUsageView(
  event: AbovePromptEvent,
  runtime: UsageRuntime,
): Promise<RenderElement> {
  const { ui, state, burnState, liveNow } = await runtime.load(event);
  const variant = state.layout === "current" ? "current" : "inline";
  runtime.setVariant(variant);
  const preview = state.preview;
  const fixture = preview
    ? usageFixture(preview.id, preview.capturedAt)
    : undefined;
  const snapshot = fixture?.snapshot ?? state.snapshot;
  const now = fixture ? preview!.capturedAt : liveNow;
  runtime.settle(liveNow);
  const windows = PRIMARY_WINDOWS.map((kind) => primaryWindow(snapshot, kind));
  const isDesktop = event.surface === "desktop" && "Svg" in ui;
  const { active: burnActive } = runtime.prepareBurn(burnState, liveNow);
  const hasReading = windows.some((window) => window.percent !== null);
  const pending = fixture?.state === "checking";
  const status = fixture?.state
    ? pending
      ? "Checking usage..."
      : "Usage unavailable"
    : snapshot.status === "error"
      ? "Usage unavailable"
      : "Usage not available yet";
  if (!hasReading && snapshot.status !== "ready") {
    runtime.empty(event);
    const hint =
      !fixture?.state && snapshot.status === "empty"
        ? "Try send a message first"
        : undefined;
    const retryHint =
      fixture?.retryAt === undefined
        ? undefined
        : `${previewRetryText(fixture.retryAt, now)} · Refresh checks session.`;
    const canRefresh =
      fixture?.state !== undefined && !pending && event.props.bodyColumns >= 24;
    return renderEmptyUsageBand({
      ui,
      event,
      isDesktop,
      status,
      hint,
      retryHint,
      canRefresh,
      statusGraphic: usageStatusSvg(status, hint, event.props.bodyColumns < 60),
      renderMark: () => runtime.renderMark(event, BRAND_MARK_SVG, "antiburn"),
      renderTerminalMark: () =>
        runtime.renderTerminalMark(event, burnState, liveNow, burnActive),
      onRefresh: () => runtime.refreshPreview("state-checking"),
    });
  }
  const columns = event.props.bodyColumns;
  if (isDesktop && columns >= 24 && "Svg" in ui) {
    const { layout, responsiveSources, mark } = runtime.prepareDesktop({
      event,
      state,
      snapshot,
      preview,
      fixture,
      windows,
      now,
      liveNow,
      variant,
    });
    const markAlt =
      burnState.previewUntil > liveNow
        ? "antiburn · output activity preview"
        : burnActive
          ? "antiburn · high recent output activity"
          : "antiburn";
    return renderDesktopUsageBand({
      ui,
      windows,
      layout,
      responsiveSources,
      now,
      status: snapshot.status,
      usageMark: mark,
      markAlt,
      renderMark: (source, alt) => runtime.renderMark(event, source, alt),
    });
  }
  runtime.cancel();
  const content = Math.max(1, columns - (isDesktop ? 3 : 12));
  const inlineColumns = Math.max(1, Math.floor((content - 3) / 2));
  const isInline =
    columns >= 80 &&
    windows.every(
      (window) =>
        window.label.length + percentage(window).length + 2 <= inlineColumns,
    );
  const meterColumns = isInline ? inlineColumns : content;
  const compact = variant === "inline";
  const geometry = (window: UsageWindow) => {
    const trackColumns = compact
      ? Math.max(
          0,
          meterColumns - window.label.length - percentage(window).length - 2,
        )
      : meterColumns;
    const count = isDesktop ? 0 : terminalDotCount(trackColumns);
    const value = pace(window, now);
    const markerCell =
      value === null || count < 2
        ? null
        : Math.max(
            1,
            Math.min(count * 2 - 3, Math.round(value * (count - 1)) * 2 - 1),
          );
    return { count, value, markerCell };
  };
  const hoverBinding =
    event.surface === "terminal"
      ? runtime.binding(
          event,
          JSON.stringify([
            columns,
            variant,
            snapshot.status,
            windows.map((window) => [
              window.resetsAt,
              percentage(window).length,
              geometry(window).markerCell,
            ]),
          ]),
        )
      : "";
  const revealed =
    event.surface === "terminal" &&
    state.hovered?.requestId === event.requestId &&
    state.hovered.binding === hoverBinding
      ? state.hovered.target
      : state.revealed;
  const pulse = runtime.pulse(
    event,
    isDesktop,
    preview,
    snapshot,
    windows,
    burnState,
    liveNow,
    fixture?.warning,
  );
  const renderMeter = (window: UsageWindow) =>
    renderUsageMeter({
      ui,
      window,
      now,
      isDesktop,
      revealed,
      geometry,
      fixtureWarning: fixture?.warning,
      pulse,
      meterColumns,
      compact,
      hoverBinding,
      reveal: runtime.reveal,
    });
  return renderCompactUsageBand({
    ui,
    isDesktop,
    isInline,
    snapshotStatus: snapshot.status,
    windows,
    renderMark: () =>
      isDesktop
        ? runtime.renderMark(event, BRAND_MARK_SVG, "antiburn")
        : runtime.renderTerminalMark(event, burnState, liveNow, burnActive),
    renderMeter,
  });
}
