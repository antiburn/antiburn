import { liveMode, type LiveMode } from "./mode";
import { enteredStages, renderChecksPaneView } from "./register-pane-view";
import { renderUsageDetailsView } from "./register-usage-details";
import {
  buildDesktopUsageAssets,
  renderUsageView,
  hoverSites,
  terminalHoverBinding,
  type DesktopAssetCache,
} from "./register-usage-view";
import { renderUsageDebugView } from "./register-debug-view";
import {
  resetUsageBurnControls,
  renderSessionModeView,
  renderUsageComposition,
} from "./register-usage-bands";
export {
  commandSummary,
  promptPreview,
  splitSentence,
} from "./register-visuals";
import {
  BRAND_MARK_SVG,
  chipLabel,
  chipView,
  toggleGroup,
  visibleRowIds,
  renderStripView,
  renderTerminalMarkView,
  renderUsageMarkView,
  renderDesktopBrandLinkView,
  centredLineView,
} from "./register-visuals";
import { atom, read, update } from "claude-code";
import {
  describe,
  parseCommand,
  renderHelp,
  renderStatus,
} from "../../../packages/antiburn-plugin-core/product.mjs";
import { renderNotice } from "../../../packages/antiburn-plugin-core/core.mjs";
import { APP_URL } from "./brand";
import type {
  EngineInterface,
  Register,
  RenderInput,
  RenderNode,
} from "claude-code";

import type {
  LiveCheckId,
  LiveCheckView,
  LivePaneState,
  LiveStripState,
  LiveViewState,
  LiveUsageState,
  LiveBurnState,
} from "../types";
import { demoView, LiveChecks, liveChecks, type LiveView } from "./checks";
import {
  applySessionChecks,
  disconnectedSessionChecks,
  parseSessionChecks,
  sessionConnectionText,
  type SessionCheckConnection,
} from "./session-checks-ui";
import { TOUR } from "./tour";
import {
  normalizeUsage,
  pace,
  percentage,
  primaryWindow,
  resetText,
  usageFailure,
  UsageOrder,
  PRIMARY_WINDOWS,
  type UsageSnapshot,
} from "./usage";
import { meterDescription } from "./usage-graphics";
import {
  EntranceOwner,
  usageLayout,
  meterGeometry,
  bezier,
} from "./usage-presentation";
import { USAGE_FIXTURES, usageFixture } from "./usage-fixtures";
import { TERMINAL_FLAME, terminalBrandCells } from "./terminal-flame";
import {
  BURN_ANIMATION_ENABLED,
  BurnActivity,
  BURN_HEIGHT,
  burnAnimationSvg,
} from "./burn";

// The validator follows $ and state references only inside this file, so every
// function that takes $ and every atom is here. checks.ts takes no $.

type PaneEvent = RenderInput<"Pane">;

const PANE = "checks";

const view = atom(
  { plugin: "antiburn-live", key: "view" } as const,
  {
    checks: [],
    resolvedLine: [],
    entered: [],
  } as LiveViewState,
);

const strip = atom(
  { plugin: "antiburn-live", key: "strip" } as const,
  {
    isDismissed: false,
    newCount: 0,
    armedId: null,
  } as LiveStripState,
);

const pane = atom(
  { plugin: "antiburn-live", key: "pane" } as const,
  {
    selectedId: null,
    openGroups: [],
    turned: null,
    fullPromptId: null,
  } as LivePaneState,
);

const usage = atom(
  { plugin: "antiburn-live", key: "usage" } as const,
  {
    snapshot: { status: "empty", windows: [], capturedAt: 0 },
    revealed: null,
    activated: null,
  } as LiveUsageState,
);
const burn = atom(
  { plugin: "antiburn-live", key: "burn" } as const,
  {
    active: false,
    previewUntil: 0,
    reduced: false,
    startedAt: 0,
    coolingAt: null,
    revision: 0,
    estimatedTps: 0,
    measuredTps: null,
    source: "unknown",
  } as LiveBurnState,
);
const burnActivity = new BurnActivity();
const burnPending = new Map<string, number>();
let burnGeneration = 0;
let burnSequence = 0;
let burnPublish: Promise<void> = Promise.resolve();
let burnObservation: Promise<void> | undefined;
let burnVisible = false;
const burnSites = new Set<string>();
let burnPreviewRevision = 0;
let burnLeaseAt = 0;
let burnSvgKey = "";
let burnSvgSource = "";
let burnPlaybackUntil = 0;
let burnCoolingStrength = 0;

type TerminalFlameSite = { revision: string; cancel: () => void };
const terminalFlames = new Map<string, TerminalFlameSite>();

const terminalPulses = new Map<
  string,
  { revision: string; startedAt: number; cancel: () => void }
>();
const TERMINAL_PULSE_HALF_MS = 800;
const TERMINAL_PULSE_DURATION_MS = 9600;

function stopTerminalPulse(requestId: string): void {
  terminalPulses.get(requestId)?.cancel();
  terminalPulses.delete(requestId);
}

function terminalPulse(
  $: EngineInterface,
  requestId: string,
  revision: string,
  now: number,
  enabled: boolean,
): boolean {
  if (!enabled) {
    stopTerminalPulse(requestId);
    return false;
  }
  if (terminalPulses.get(requestId)?.revision !== revision) {
    stopTerminalPulse(requestId);
    const site = { revision, startedAt: now, cancel: () => {} };
    terminalPulses.set(requestId, site);
    const schedule = () => {
      const timer = $.clock.after(TERMINAL_PULSE_HALF_MS, () => {
        void (async () => {
          try {
            const time = await $.clock.now();
            if (terminalPulses.get(requestId) !== site) return;
            $.ui.invalidate("ui.render");
            // Keep the completed revision so a redraw cannot restart the pulse.
            if (time - site.startedAt < TERMINAL_PULSE_DURATION_MS) schedule();
          } catch {
            if (terminalPulses.get(requestId) === site)
              stopTerminalPulse(requestId);
          }
        })();
      });
      site.cancel = () => timer.cancel();
    };
    schedule();
  }
  const elapsed = now - terminalPulses.get(requestId)!.startedAt;
  return (
    elapsed < TERMINAL_PULSE_DURATION_MS &&
    Math.floor(elapsed / TERMINAL_PULSE_HALF_MS) % 2 === 1
  );
}

function stopTerminalFlame(requestId: string): void {
  terminalFlames.get(requestId)?.cancel();
  terminalFlames.delete(requestId);
}

function resetTerminalFlames(): void {
  for (const requestId of terminalPulses.keys()) stopTerminalPulse(requestId);
  for (const requestId of terminalFlames.keys()) stopTerminalFlame(requestId);
}

function animateTerminalFlame(
  $: EngineInterface,
  e: RenderInput<"AbovePrompt">,
  state: LiveBurnState,
  now: number,
  active: boolean,
): string {
  if (!BURN_ANIMATION_ENABLED) return terminalBrandCells(0, 0);
  const until = active
    ? state.previewUntil > now
      ? state.previewUntil
      : burnLeaseAt + 8000
    : state.coolingAt === null
      ? now
      : state.coolingAt + TERMINAL_FLAME.fadeMs;
  const revision = `${e.props.bodyColumns}:${state.startedAt}:${until}:${state.reduced}`;
  const cellsAt = (time: number) =>
    terminalBrandCells(
      Math.max(0, time - state.startedAt),
      state.reduced
        ? time < until
          ? 1
          : 0
        : Math.max(0, Math.min(1, (until - time) / TERMINAL_FLAME.fadeMs)),
      state.reduced,
    );
  if (now >= until) {
    stopTerminalFlame(e.requestId);
    return terminalBrandCells(0, 0);
  }
  if (terminalFlames.get(e.requestId)?.revision !== revision) {
    stopTerminalFlame(e.requestId);
    const site: TerminalFlameSite = { revision, cancel: () => {} };
    terminalFlames.set(e.requestId, site);
    const current = () => terminalFlames.get(e.requestId) === site;
    const paint = async (): Promise<void> => {
      if (!current()) return;
      try {
        const time = await $.clock.now();
        if (!current()) return;
        const result = await $.ui.blit({
          requestId: e.requestId,
          key: "usage-terminal-flame",
          columns: 10,
          rows: 1,
          cells: cellsAt(time).slice(0, 160),
        });
        if (!current()) return;
        $.ui.invalidate("ui.render");
        if (result.deny || time >= until) {
          stopTerminalFlame(e.requestId);
          return;
        }
        schedule(state.reduced ? until - time : TERMINAL_FLAME.frameMs);
      } catch {
        if (current()) stopTerminalFlame(e.requestId);
      }
    };
    const schedule = (ms: number) => {
      const timer = $.clock.after(Math.max(1, ms), () => {
        void paint();
      });
      site.cancel = () => timer.cancel();
    };
    schedule(state.reduced ? until - now : TERMINAL_FLAME.frameMs);
  }
  return cellsAt(now);
}

function resetBurn(): void {
  resetTerminalFlames();
  burnGeneration += 1;
  burnPreviewRevision += 1;
  burnSites.clear();
  burnActivity.reset();
  burnPending.clear();
  burnPublish = Promise.resolve();
  burnObservation = undefined;
  burnVisible = false;
  burnLeaseAt = 0;
  burnPlaybackUntil = 0;
  burnCoolingStrength = 0;
  burnSvgKey = "";
  resetUsageBurnControls();
}

async function resetBurnState($: EngineInterface): Promise<void> {
  await update($, burn, (current) => ({
    ...current,
    active: false,
    previewUntil: 0,
    reduced: false,
    coolingAt: null,
    estimatedTps: 0,
    measuredTps: null,
    source: "idle",
    revision: current.revision + 1,
  }));
}

async function tickBurn(
  $: EngineInterface,
  generation: number,
  force = false,
): Promise<void> {
  const publish = async (): Promise<void> => {
    if (generation !== burnGeneration) return;
    const previewRevision = burnPreviewRevision;
    const now = await $.clock.now();
    if (generation !== burnGeneration) return;
    for (const [id, count] of burnPending) {
      if (count > 0) burnActivity.chunk(id, count, now);
      burnPending.set(id, 0);
    }
    const snapshot = burnActivity.snapshot(now);
    const prior = await read($, burn);
    if (
      generation !== burnGeneration ||
      previewRevision !== burnPreviewRevision
    )
      return;
    const previewUntil = prior.previewUntil > now ? prior.previewUntil : 0;
    const active = burnVisible && (previewUntil > 0 || snapshot.active);
    const changed = active !== prior.active;
    const renew = active && previewUntil === 0 && now - burnLeaseAt >= 4000;
    if (changed || renew || previewUntil !== prior.previewUntil || force) {
      const reenter = active && (!prior.active || now - burnLeaseAt >= 8000);
      if (changed && !active) {
        burnCoolingStrength = Math.max(
          0,
          Math.min(
            1,
            (burnPlaybackUntil - now) / 800,
            (now - prior.startedAt) / 220,
          ),
        );
      }
      if (changed || renew) burnLeaseAt = now;
      await update($, burn, (current) =>
        generation !== burnGeneration || previewRevision !== burnPreviewRevision
          ? current
          : {
              ...current,
              active,
              previewUntil,
              reduced: active
                ? previewUntil > 0 && current.reduced
                : current.reduced,
              startedAt: reenter ? now : current.startedAt,
              coolingAt:
                !active && prior.active
                  ? now
                  : active
                    ? null
                    : current.coolingAt,
              revision:
                changed || renew || previewUntil !== prior.previewUntil
                  ? current.revision + 1
                  : current.revision,
              estimatedTps: snapshot.estimatedTps,
              measuredTps: snapshot.measuredTps,
              source: snapshot.source,
            },
      );
    }
  };
  const pending = burnPublish.then(publish, publish);
  burnPublish = pending.catch(() => {});
  await pending;
}

function observeBurn($: EngineInterface, generation: number): void {
  if (generation !== burnGeneration || burnObservation) return;
  const pending = tickBurn($, generation);
  burnObservation = pending;
  void pending
    .catch(() => {})
    .finally(() => {
      if (burnObservation === pending) burnObservation = undefined;
    });
}

async function previewBurn(
  $: EngineInterface,
  reduced = false,
  duration = 15000,
  sequenceGeneration?: number,
): Promise<void> {
  if (!BURN_ANIMATION_ENABLED) return;
  const generation = burnGeneration;
  const revision = ++burnPreviewRevision;
  const now = await $.clock.now();
  if (
    generation !== burnGeneration ||
    revision !== burnPreviewRevision ||
    (sequenceGeneration !== undefined &&
      sequenceGeneration !== usageSequenceGeneration)
  )
    return;
  burnLeaseAt = now;
  await update($, burn, (current) =>
    generation !== burnGeneration ||
    revision !== burnPreviewRevision ||
    (sequenceGeneration !== undefined &&
      sequenceGeneration !== usageSequenceGeneration)
      ? current
      : {
          ...current,
          active: true,
          previewUntil: now + duration,
          reduced,
          startedAt: now,
          coolingAt: null,
          revision: current.revision + 1,
        },
  );
}

async function stopBurnPreview($: EngineInterface): Promise<void> {
  if (!BURN_ANIMATION_ENABLED) return;
  burnPreviewRevision += 1;
  await update($, burn, (current) => ({ ...current, previewUntil: 0 }));
  await tickBurn($, burnGeneration);
}

async function refreshBurnStats($: EngineInterface): Promise<void> {
  if (!BURN_ANIMATION_ENABLED) return;
  const generation = burnGeneration;
  const now = await $.clock.now();
  if (generation !== burnGeneration) return;
  const snapshot = burnActivity.snapshot(now);
  await update($, burn, (current) =>
    generation !== burnGeneration
      ? current
      : {
          ...current,
          estimatedTps: snapshot.estimatedTps,
          measuredTps: snapshot.measuredTps,
          source: snapshot.source,
        },
  );
}

let mode: LiveMode = "usage";
const usageOrder = new UsageOrder();
const DEBUG_PANE = "debug";
let needsUsageReconciliation = false;
let usageUiEnabled = true;
let releaseOwnsUsage = false;
let releaseOwnershipResolved = false;
let startupNoticeEnabled = false;
let startupNoticeSessionId: string | undefined;
const usageEntrance = new EntranceOwner();
let usageMount: string | undefined;
let usageRenderKey: string | undefined;
let usageSources: string[] = [];
let usageMark = "";
let usageMarkEntranceAt: number | null | undefined;
let usageMarkReplayRevision = -1;
let usagePreviewRevision = 0;
let usageReplayRevision = 0;
let usageSequenceGeneration = 0;
let usageSequenceTimer: { cancel: () => void } | undefined;
let usageSequenceEmber = false;
let usageMeterWidth = 0;
let usageLayoutVariant: "inline" | "current" = "inline";
let usagePreviewFill:
  | { from: number[]; to: number[]; startedAt: number }
  | undefined;
let previousUsage: UsageSnapshot | undefined;
let usageRoll: { previous: UsageSnapshot; startedAt: number } | undefined;

function cancelUsagePresentation(): void {
  if (usageEntrance.state.phase === "running") {
    usageMark = BRAND_MARK_SVG;
    usageMarkEntranceAt = null;
  }
  usageEntrance.cancel();
  usagePreviewFill = undefined;
  usageRoll = undefined;
  usageRenderKey = undefined;
}

// CSS settles the visuals. Host events release expired bookkeeping without timers.
function settleUsagePresentation(now: number): void {
  const startedAt = usageEntrance.state.startedAt;
  if (startedAt !== null && now - startedAt >= 960) usageEntrance.cancel();
  if (usageRoll !== undefined && now - usageRoll.startedAt >= 284)
    usageRoll = undefined;
}

function checksEnabled(): boolean {
  return mode === "checks";
}

async function clearDisabledChecks($: EngineInterface): Promise<void> {
  if (checksEnabled()) return;
  const previous = await read($, view);
  if (previous.checks.length > 0 || previous.resolvedLine.length > 0) {
    await update($, view, () => ({
      checks: [],
      resolvedLine: [],
      entered: [],
    }));
    await update($, strip, () => ({
      isDismissed: false,
      newCount: 0,
      armedId: null,
    }));
    await update($, pane, () => ({
      selectedId: null,
      openGroups: [],
      turned: null,
      fullPromptId: null,
    }));
    await $.ui.status(undefined);
  }
  const panes = await $.ui.panes();
  if (panes.some((open) => open.id === PANE)) await $.ui.close({ id: PANE });
}

async function commitUsage(
  $: EngineInterface,
  snapshot: UsageSnapshot,
  ticket: { generation: number; revision: number },
) {
  if (!usageOrder.isCurrent(ticket)) return;
  if ((await read($, usage)).preview) {
    previousUsage = snapshot;
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket) ? { ...current, snapshot } : current,
    );
    return;
  }
  if (
    previousUsage?.status === snapshot.status &&
    JSON.stringify(previousUsage.windows) ===
      JSON.stringify(snapshot.windows) &&
    PRIMARY_WINDOWS.every((kind) => {
      const window = primaryWindow(snapshot, kind);
      return (
        resetText(window, previousUsage!.capturedAt) ===
          resetText(window, snapshot.capturedAt) &&
        pace(window, previousUsage!.capturedAt) ===
          pace(window, snapshot.capturedAt)
      );
    })
  ) {
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket)
        ? {
            ...current,
            snapshot: { ...snapshot, capturedAt: current.snapshot.capturedAt },
          }
        : current,
    );
    return;
  }
  settleUsagePresentation(snapshot.capturedAt);
  const wasEntering = usageEntrance.state.phase === "running";
  if (wasEntering) cancelUsagePresentation();
  else if (usageEntrance.state.phase === "consumed") {
    cancelUsagePresentation();
    if (
      previousUsage !== undefined &&
      snapshot.status === "ready" &&
      previousUsage.status === "ready" &&
      PRIMARY_WINDOWS.some(
        (kind) =>
          percentage(primaryWindow(previousUsage!, kind)) !==
          percentage(primaryWindow(snapshot, kind)),
      )
    ) {
      usageRoll = { previous: previousUsage, startedAt: snapshot.capturedAt };
    }
  }
  previousUsage = snapshot;
  usageRenderKey = undefined;
  await update($, usage, (current) =>
    usageOrder.isCurrent(ticket) ? { ...current, snapshot } : current,
  );
}

async function readUsage($: EngineInterface, explicit = false): Promise<void> {
  if (!usageUiEnabled && !explicit) return;
  const ticket = usageOrder.beginRead();
  try {
    const [reading, now] = await Promise.all([
      $.session.usage(),
      $.clock.now(),
    ]);
    await commitUsage($, normalizeUsage(reading.rateLimits, now), ticket);
  } catch {
    if (!usageOrder.isCurrent(ticket)) return;
    if (!(await read($, usage)).preview) cancelUsagePresentation();
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket)
        ? {
            ...current,
            snapshot: usageFailure(
              current.snapshot,
              current.snapshot.capturedAt,
            ),
          }
        : current,
    );
  }
}

async function revealUsage(
  $: EngineInterface,
  kind: string | null,
): Promise<void> {
  await update($, usage, (current) => {
    const dismissed = current.activated === kind;
    return {
      ...current,
      revealed: dismissed ? null : kind,
      activated: !dismissed ? kind : null,
    };
  });
}

async function reconcileUsage($: EngineInterface): Promise<void> {
  if (!usageUiEnabled || releaseOwnsUsage || !needsUsageReconciliation) return;
  needsUsageReconciliation = false;
  await readUsage($);
  await publish($);
}

function cancelUsageSequence(): void {
  usageSequenceGeneration += 1;
  usageSequenceTimer?.cancel();
  usageSequenceTimer = undefined;
}

async function stopUsageSequence($: EngineInterface): Promise<void> {
  const ember = usageSequenceEmber;
  usageSequenceEmber = false;
  cancelUsageSequence();
  usagePreviewRevision += 1;
  await update($, usage, (current) => ({ ...current, sequence: undefined }));
  if (ember) await stopBurnPreview($);
}

async function startUsageSequence($: EngineInterface): Promise<void> {
  cancelUsageSequence();
  usagePreviewRevision += 1;
  const generation = usageSequenceGeneration;
  const steps = [
    ...USAGE_FIXTURES.filter(
      (fixture) =>
        !["nine", "ten", "ninety-nine", "hundred"].includes(fixture.id),
    ).map((fixture, index) => ({
      label: index === 0 ? "Entrance · " + fixture.label : fixture.label,
      id: fixture.id,
      action: index === 0 ? ("entrance" as const) : undefined,
      hold: fixture.id === "near-limit" ? 10000 : 1800,
    })),
    { label: "Reset reveal", action: "show" as const, hold: 1400 },
    { label: "Reset hide", action: "hide" as const, hold: 500 },
    { label: "Pace reveal", action: "pace-show" as const, hold: 1400 },
    { label: "Pace hide", action: "pace-hide" as const, hold: 500 },
    ...(BURN_ANIMATION_ENABLED
      ? [
          {
            label: "Ember catch · High usage · 88% / 82%",
            id: "high",
            ember: true,
            hold: 4000,
          },
        ]
      : []),
  ];
  const advance = async (index: number): Promise<void> => {
    if (generation !== usageSequenceGeneration) return;
    const step = steps[index];
    if (!step) {
      if (generation === usageSequenceGeneration) await stopUsageSequence($);
      return;
    }
    if ("id" in step && step.id) await selectUsagePreview($, step.id, true);
    if (generation !== usageSequenceGeneration) return;
    if ("action" in step && step.action)
      await inspectUsageAnimation($, step.action, true);
    if (generation !== usageSequenceGeneration) return;
    if ("ember" in step) {
      usageSequenceEmber = true;
      await previewBurn($, false, step.hold, generation);
    }
    if (generation !== usageSequenceGeneration) return;
    await update($, usage, (current) =>
      generation === usageSequenceGeneration
        ? {
            ...current,
            sequence: `${index + 1}/${steps.length} · ${step.label}`,
          }
        : current,
    );
    if (generation !== usageSequenceGeneration) return;
    usageSequenceTimer = $.clock.after(step.hold, () => {
      void advance(index + 1).catch(async () => {
        if (generation === usageSequenceGeneration) await stopUsageSequence($);
      });
    });
  };
  await advance(0).catch(async () => {
    if (generation === usageSequenceGeneration) await stopUsageSequence($);
  });
}

async function selectUsagePreview(
  $: EngineInterface,
  id: string | null,
  sequence = false,
): Promise<void> {
  if (!sequence) await stopUsageSequence($);
  const revision = ++usagePreviewRevision;
  const now = await $.clock.now();
  if (id !== null && usageFixture(id, now) === undefined) return;
  const prior = await read($, usage);
  if (revision !== usagePreviewRevision) return;
  const before = prior.preview
    ? usageFixture(prior.preview.id, prior.preview.capturedAt)?.snapshot
    : prior.snapshot;
  const target = id === null ? undefined : usageFixture(id, now)?.snapshot;
  const inFlight = usagePreviewFill;
  const progress = inFlight
    ? bezier((now - inFlight.startedAt) / 360, 0.23, 1, 0.32, 1)
    : 1;
  const from = inFlight
    ? inFlight.from.map(
        (start, index) => start + (inFlight.to[index]! - start) * progress,
      )
    : PRIMARY_WINDOWS.map(
        (kind) =>
          meterGeometry(
            primaryWindow(before!, kind),
            usageMeterWidth,
            now,
            usageLayoutVariant,
          ).fillEnd,
      );
  cancelUsagePresentation();
  if (target?.status === "ready" && usageMeterWidth > 0)
    usagePreviewFill = {
      from,
      to: PRIMARY_WINDOWS.map(
        (kind) =>
          meterGeometry(
            primaryWindow(target, kind),
            usageMeterWidth,
            now,
            usageLayoutVariant,
          ).fillEnd,
      ),
      startedAt: now,
    };
  if (
    before?.status === "ready" &&
    id !== null &&
    usageFixture(id, now)?.snapshot.status === "ready"
  )
    usageRoll = { previous: before, startedAt: now };
  await update($, usage, (current) =>
    revision !== usagePreviewRevision
      ? current
      : {
          ...current,
          sequence: sequence ? current.sequence : undefined,
          previewReveal: undefined,
          preview: id === null ? null : { id, capturedAt: now },
          revealed: null,
          activated: null,
        },
  );
}

async function replayUsageEntrance($: EngineInterface): Promise<void> {
  const current = await read($, usage);
  const snapshot = current.preview
    ? usageFixture(current.preview.id, current.preview.capturedAt)?.snapshot
    : current.snapshot;
  if (
    snapshot?.status !== "ready" ||
    !snapshot.windows.some((window) => window.percent !== null)
  )
    await selectUsagePreview($, "low");
  await inspectUsageAnimation($, "entrance");
}

async function inspectUsageAnimation(
  $: EngineInterface,
  action: "entrance" | "show" | "hide" | "pace-show" | "pace-hide",
  sequence = false,
): Promise<void> {
  if (!sequence) await stopUsageSequence($);
  const revision = ++usagePreviewRevision;
  const current = await read($, usage);
  if (!current.preview && action !== "entrance") return;
  const now = await $.clock.now();
  if (revision !== usagePreviewRevision) return;
  cancelUsagePresentation();
  if (action === "entrance") {
    usageReplayRevision += 1;
    usageEntrance.load(true);
  }
  await update($, usage, (state) =>
    revision !== usagePreviewRevision
      ? state
      : {
          ...state,
          sequence: sequence ? state.sequence : undefined,
          revealed:
            action === "show"
              ? "five_hour"
              : action === "pace-show"
                ? "pace-five_hour"
                : null,
          activated: null,
          preview: current.preview
            ? { ...current.preview, capturedAt: now }
            : current.preview,
          previewReveal: action === "entrance" ? undefined : action,
        },
  );
}

async function renderUsageDetails($: EngineInterface, e: PaneEvent) {
  const state = await read($, usage);
  const preview = state.preview;
  const fixture = preview
    ? usageFixture(preview.id, preview.capturedAt)
    : undefined;
  const snapshot = fixture?.snapshot ?? state.snapshot;
  const now = fixture ? preview!.capturedAt : await $.clock.now();
  return renderUsageDetailsView(
    $.ui.resolve(e),
    snapshot,
    now,
    fixture !== undefined,
    () => readUsage($, true),
  );
}

async function showCheckDemo($: EngineInterface): Promise<void> {
  if (!checksEnabled()) return;
  invalidateSessionCheckReads();
  isDemo = true;
  tour = null;
  const checks = demoView();
  await update($, view, () => ({
    checks,
    resolvedLine: [],
    entered: [...new Set(checks.map((check) => check.stage))],
  }));
  await showStatus($, checks);
  await openPane($);
}

async function setDebugMode(
  $: EngineInterface,
  nextMode: LiveMode,
): Promise<void> {
  if (mode === nextMode) return;
  invalidateSessionCheckReads();
  mode = nextMode;
  const generation = sessionCheckGeneration;
  const isCurrent = () =>
    sessionCheckGeneration === generation && mode === nextMode && checksEnabled();
  resetTracker();
  await clearDisabledChecks($);
  if (checksEnabled()) {
    await restoreSessionChecks($, isCurrent);
    if (!isCurrent()) return;
    await showStatus($, (await read($, view)).checks, isCurrent);
    if (!isCurrent()) return;
    let sessionId: string | undefined;
    try {
      sessionId = await $.session.id();
    } catch {
      sessionId = undefined;
    }
    await refreshSessionChecks($, sessionId);
  }
  else await $.ui.status(undefined);
  const registered = await $.command.register({
    name: "antiburn-dev",
    description: checksEnabled()
      ? "Show live checks"
      : "Usage limits are shown above the prompt",
    argumentHint: checksEnabled()
      ? "[checks | demo | demo tour | demo off]"
      : undefined,
  });
  commandName = registered.command;
  await update($, usage, (current) => ({ ...current }));
}

async function selectUsageLayout(
  $: EngineInterface,
  layout: "inline" | "current",
): Promise<void> {
  resetTerminalFlames();
  await stopUsageSequence($);
  usagePreviewRevision += 1;
  cancelUsagePresentation();
  usageMount = undefined;
  usageMeterWidth = 0;
  usageLayoutVariant = layout;
  await update($, usage, (current) => ({
    ...current,
    layout,
    sequence: undefined,
    previewReveal: undefined,
    revealed: null,
    activated: null,
  }));
}

async function renderUsageDebug($: EngineInterface, e: PaneEvent) {
  const ui = $.ui.resolve(e);
  const state = await read($, usage);
  const preview = state.preview;
  const fixture = preview
    ? usageFixture(preview.id, preview.capturedAt)
    : undefined;
  const burnState = await read($, burn);
  const usageDetails = await renderUsageDetails($, e);
  return renderUsageDebugView(
    ui,
    e,
    state,
    preview,
    fixture,
    burnState,
    checksEnabled(),
    usageDetails,
    {
      setDebugMode: (mode) => setDebugMode($, mode),
      openPane: async () => {
        await openPane($);
      },
      showCheckDemo: () => showCheckDemo($),
      startTour: () => startTour($),
      endDemo: () => endDemo($),
      previewBurn: (reduced) => previewBurn($, reduced),
      stopBurnPreview: () => stopBurnPreview($),
      refreshBurnStats: () => refreshBurnStats($),
      selectUsageLayout: (layout) => selectUsageLayout($, layout),
      selectUsagePreview: (id) => selectUsagePreview($, id),
      selectUsageState: (state) => selectUsagePreview($, `state-${state}`),
      replayUsageEntrance: () => replayUsageEntrance($),
      startUsageSequence: () => startUsageSequence($),
      stopUsageSequence: () => stopUsageSequence($),
      toggleSyntheticLabels: async () => {
        await update($, usage, (current) => ({
          ...current,
          showSyntheticLabels: !current.showSyntheticLabels,
        }));
      },
      inspectUsageAnimation: (action) => inspectUsageAnimation($, action),
    },
  );
}

async function openTerminalWebsite($: EngineInterface): Promise<void> {
  const windows = (await $.env.get("OS")) === "Windows_NT";
  const commands = windows
    ? [["cmd.exe", "/c", "start", "", APP_URL]]
    : [
        ["open", APP_URL],
        ["xdg-open", APP_URL],
      ];
  for (const argv of commands) {
    try {
      const result = await $.process.run(argv, { timeoutMs: 5000 });
      if (result.exitCode === 0) return;
    } catch {
      continue;
    }
  }
  await $.ui.status(`Open ${APP_URL} in your browser`);
}

function renderTerminalMark(
  $: EngineInterface,
  e: RenderInput<"AbovePrompt">,
  state: LiveBurnState,
  now: number,
  active: boolean,
): RenderNode {
  const ui = $.ui.resolve(e);
  /* Braille reference: keep inactive because terminal line spacing splits the mark.
  if ('Raster' in ui) {
    // Adjust only the empty padding to center the grid beside one or two meter rows.
    const inlineCells = warm ? 'gCgAAGKu/wAAAAABwCgAAGKu/wAAAAABIAAAAGKu/wAAAAABlCgAAGKu/wAAAAABUigAAGKu/wAAAAABRygAAGKu/wAAAAABIAAAAGKu/wAAAAABIAAAAGKu/wAAAAABIAAAAGKu/wAAAAAB' : 'gCgAABRZ/wAAAAABwCgAABRZ/wAAAAABIAAAABRZ/wAAAAABlCgAABRZ/wAAAAABUigAABRZ/wAAAAABRygAABRZ/wAAAAABIAAAABRZ/wAAAAABIAAAABRZ/wAAAAABIAAAABRZ/wAAAAAB'
    const stackedCells = warm ? 'IAAAAGKu/wAAAAABIAAAAGKu/wAAAAABIAAAAGKu/wAAAAABkCgAAGKu/wAAAAAB0igAAGKu/wAAAAABRCgAAGKu/wAAAAABESgAAGKu/wAAAAABAigAAGKu/wAAAAABAygAAGKu/wAAAAABIAAAAGKu/wAAAAABIAAAAGKu/wAAAAABIAAAAGKu/wAAAAAB' : 'IAAAABRZ/wAAAAABIAAAABRZ/wAAAAABIAAAABRZ/wAAAAABkCgAABRZ/wAAAAAB0igAABRZ/wAAAAABRCgAABRZ/wAAAAABESgAABRZ/wAAAAABAigAABRZ/wAAAAABAygAABRZ/wAAAAABIAAAABRZ/wAAAAABIAAAABRZ/wAAAAABIAAAABRZ/wAAAAAB'
    return <ui.Raster key="usage-brand-raster" columns={3} rows={stacked ? 4 : 3} cells={stacked ? stackedCells : inlineCells} />
  }
  */
  const cells =
    "Raster" in ui ? animateTerminalFlame($, e, state, now, active) : undefined;
  return renderTerminalMarkView(ui, cells, () => openTerminalWebsite($));
}

function renderDesktopBrandLink(
  $: EngineInterface,
  e: RenderInput<"AbovePrompt">,
): RenderNode {
  return renderDesktopBrandLinkView($.ui.resolve(e));
}

function renderUsageMark(
  $: EngineInterface,
  e: RenderInput<"AbovePrompt">,
  resting: string,
  alt: string,
): RenderNode | null {
  const ui = $.ui.resolve(e);
  if (!("Svg" in ui)) return null;
  return renderUsageMarkView(ui, resting, alt, burnSvgSource, BURN_HEIGHT, () =>
    renderDesktopBrandLink($, e),
  );
}

function renderUsage($: EngineInterface, e: RenderInput<"AbovePrompt">) {
  return renderUsageView(e, {
    load: async (event) => {
      const ui = $.ui.resolve(event);
      const state = await read($, usage);
      usageLayoutVariant = state.layout === "current" ? "current" : "inline";
      const burnState = await read($, burn);
      const liveNow = await $.clock.now();
      return { ui, state, burnState, liveNow };
    },
    settle: settleUsagePresentation,
    setVariant: (variant) => {
      usageLayoutVariant = variant;
    },
    prepareBurn: (burnState, liveNow) => {
      const previewActive =
        BURN_ANIMATION_ENABLED && burnState.previewUntil > liveNow;
      const burnActive =
        BURN_ANIMATION_ENABLED &&
        burnVisible &&
        burnState.active &&
        (previewActive ||
          (burnState.previewUntil === 0 && liveNow - burnLeaseAt < 8000));
      const burnCooling =
        BURN_ANIMATION_ENABLED &&
        burnCoolingStrength > 0 &&
        burnState.coolingAt !== null &&
        liveNow - burnState.coolingAt < 800;
      const burnKey = `${burnState.revision}:${burnState.reduced}:${burnActive}:${burnCooling}`;
      if (burnSvgKey !== burnKey) {
        if (!burnActive && burnSvgSource) usageRenderKey = undefined;
        const lease = previewActive
          ? Math.max(800, burnState.previewUntil - liveNow)
          : 8000;
        if (burnActive) burnPlaybackUntil = liveNow + lease;
        burnSvgSource =
          burnActive || burnCooling
            ? burnAnimationSvg(
                Math.max(0, liveNow - burnState.startedAt),
                !burnActive,
                lease,
                burnCoolingStrength,
              )
            : "";
        if (burnState.reduced) {
          burnSvgSource = burnSvgSource.replace(
            "<style>",
            "<style>.motion{display:none!important}.reduced{display:inline!important}",
          );
        }
        burnSvgKey = burnKey;
      }
      return {
        active: burnActive,
        revealExpires: previewActive
          ? burnState.previewUntil
          : burnLeaseAt + 8000,
      };
    },
    prepareDesktop: ({
      event,
      state,
      snapshot,
      preview,
      fixture,
      windows,
      now,
      liveNow,
      variant,
    }) => {
      const layout = usageLayout(event.props.bodyColumns * 8, variant);
      usageMeterWidth = layout.meterWidth;
      const mount = `${event.requestId}:${event.props.bodyColumns}:${event.props.maxRows}:${variant}`;
      if (usageMount !== undefined && usageMount !== mount)
        cancelUsagePresentation();
      usageMount = mount;
      usageEntrance.claim(
        liveNow,
        event.props.maxRows > 0,
        snapshot.status === "ready" &&
          windows.some((window) => window.percent !== null),
      );
      const cache: DesktopAssetCache = {
        key: usageRenderKey,
        sources: usageSources,
        mark: usageMark,
        markEntranceAt: usageMarkEntranceAt,
        markReplayRevision: usageMarkReplayRevision,
      };
      const assets = buildDesktopUsageAssets({
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
        entranceStartedAt: usageEntrance.state.startedAt,
        roll: usageRoll,
        previewFill: usagePreviewFill,
        replayRevision: usageReplayRevision,
        cache,
      });
      usageSources = assets.sources;
      usageMark = assets.mark;
      usageMarkEntranceAt = assets.markEntranceAt;
      usageMarkReplayRevision = assets.markReplayRevision;
      usageRenderKey = assets.key;
      return { layout, sources: usageSources, mark: usageMark };
    },
    empty: (event) => {
      if (event.surface === "terminal")
        terminalHoverBinding(event.requestId, "empty");
      stopTerminalPulse(event.requestId);
    },
    cancel: () => {
      if (usageEntrance.state.phase !== "consumed") cancelUsagePresentation();
    },
    binding: (event, shape) => terminalHoverBinding(event.requestId, shape),
    pulse: (
      event,
      isDesktop,
      preview,
      snapshot,
      windows,
      burnState,
      now,
      warning,
    ) =>
      terminalPulse(
        $,
        event.requestId,
        JSON.stringify([preview, snapshot.windows, burnState.reduced]),
        now,
        !isDesktop &&
          !burnState.reduced &&
          warning !== undefined &&
          windows.some((window) => {
            const value = pace(window, now);
            return (
              window.percent !== null &&
              window.percent > 0 &&
              (warning === "pulse" ||
                (value !== null && window.percent / 100 > value))
            );
          }),
      ),
    renderMark: (event, source, alt) => renderUsageMark($, event, source, alt),
    renderTerminalMark: (event, burnState, now, active) =>
      renderTerminalMark($, event, burnState, now, active),
    refreshPreview: (id) => selectUsagePreview($, id),
    reveal: (kind) => revealUsage($, kind),
  });
}

let tracker = new LiveChecks();
// While a demo shows, publish leaves the sample checks in place until /antiburn-dev demo off.
let isDemo = false;
let sessionCheckCache: LiveViewState | undefined;
let sessionCheckSessionId: string | undefined;
let sessionCheckGeneration = 0;
let sessionCheckReadSequence = 0;

function invalidateSessionCheckReads(): void {
  sessionCheckGeneration += 1;
  sessionCheckReadSequence += 1;
}

/**
 * The permission mode comes from the classic hook input, which carries
 * permission_mode at each prompt and at each turn end. The SessionMode slot
 * shows only footer labels, not the permission mode. Before the first value
 * arrives, the mod asks for two presses.
 */
let isUnattended = true;

/**
 * The demo tour: a second tracker that the tour steps feed, and the number of steps done.
 * The real hooks keep feeding the live tracker, so the tour never changes the live checks.
 */
let tour: { checks: LiveChecks; beat: number } | null = null;

// After the person closes the pane by hand, a new check does not open it again in this session.
let isPaneClosedByPerson = false;

/** The tracker that the strip, the chip and the pane show: the tour tracker while the tour runs. */
function shownTracker(): LiveChecks {
  return tour?.checks ?? tracker;
}

function resetTracker(): void {
  tracker = new LiveChecks();
  isUnattended = true;
  isDemo = false;
  tour = null;
  isPaneClosedByPerson = false;
}

async function resetSessionChecks(
  $: EngineInterface,
  sessionId?: string,
  activeReadSequence?: number,
): Promise<void> {
  if (activeReadSequence === undefined) invalidateSessionCheckReads();
  else sessionCheckGeneration += 1;
  sessionCheckSessionId = sessionId;
  sessionCheckCache = undefined;
  await update($, view, () => ({
    checks: [],
    resolvedLine: [],
    entered: [],
    connection: "waiting" as const,
  }));
  await update($, strip, () => ({ isDismissed: false, newCount: 0, armedId: null }));
}

async function setSessionConnection(
  $: EngineInterface,
  connection: SessionCheckConnection,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  const current = await read($, view);
  if (!isCurrent()) return;
  const next = disconnectedSessionChecks(current);
  await update($, view, (latest) => isCurrent() ? { ...next, connection } : latest);
  if (!isCurrent()) return;
  sessionCheckCache = { ...next, connection };
}

async function refreshSessionChecks(
  $: EngineInterface,
  sessionId: string | undefined,
): Promise<void> {
  if (!checksEnabled() || isDemo || sessionId === undefined) return;
  const sequence = ++sessionCheckReadSequence;
  if (sessionCheckSessionId !== sessionId)
    await resetSessionChecks($, sessionId, sequence);
  const generation = sessionCheckGeneration;
  const isCurrent = () =>
    sessionCheckGeneration === generation &&
    sessionCheckReadSequence === sequence &&
    sessionCheckSessionId === sessionId &&
    checksEnabled() &&
    !isDemo;
  let result;
  try {
    result = await $.process.run(
      ["antiburn", "checks", sessionId, "--delivery-v1", "--consumer", "claude-code:ui"],
      { timeoutMs: 1500 },
    );
  } catch {
    if (isCurrent()) await setSessionConnection($, "disconnected", isCurrent);
    return;
  }
  if (!isCurrent()) return;
  if (
    result.exitCode !== 0 || result.isStdoutTruncated || result.stdout.length > 256 * 1024
  ) {
    if (isCurrent()) await setSessionConnection($, "disconnected", isCurrent);
    return;
  }
  const delivery = parseSessionChecks(result.stdout, sessionId);
  if (delivery === undefined) {
    if (isCurrent()) await setSessionConnection($, "disconnected", isCurrent);
    return;
  }
  const current = await read($, view);
  if (!isCurrent()) return;
  const next = applySessionChecks(current, delivery);
  if (!isCurrent()) return;
  await update($, view, (latest) => isCurrent() ? next : latest);
  if (!isCurrent()) return;
  sessionCheckCache = next;
  await showStatus($, next.checks, isCurrent);
  if (!isCurrent()) return;
  if (delivery.receipt !== null) {
    if (!isCurrent()) return;
    try {
      const ack = await $.process.run(
        ["antiburn", "checks", sessionId, "--delivery-v1", "--consumer", "claude-code:ui", "--ack", delivery.receipt],
        { timeoutMs: 1500 },
      );
      if (!isCurrent() || ack.exitCode !== 0 || ack.isStdoutTruncated) return;
      const accepted = JSON.parse(ack.stdout) as { protocol_version?: unknown; acknowledged?: unknown };
      if (isCurrent() && (accepted.protocol_version !== 1 || accepted.acknowledged !== true))
        await $.ui.log("Antiburn checks were shown, but the delivery receipt was not accepted.", { to: "debug" });
    } catch {
      if (isCurrent())
        await $.ui.log("Antiburn checks were shown, but the delivery receipt was not accepted.", { to: "debug" });
    }
  }
}

async function restoreSessionChecks(
  $: EngineInterface,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  if (!isCurrent()) return;
  const cached = sessionCheckCache;
  const next = cached ?? {
    checks: [],
    resolvedLine: [],
    entered: [],
    connection: "waiting" as const,
  };
  await update($, view, (current) => isCurrent() ? next : current);
  if (!isCurrent()) return;
  await showStatus($, next.checks, isCurrent);
}

function isUnattendedMode(): boolean {
  return isUnattended;
}

function noteMode(mode: string | undefined): void {
  if (mode === undefined) return;
  isUnattended = mode === "auto" || mode === "bypassPermissions";
}

let commandName = "antiburn-dev";

async function renderStrip(
  $: EngineInterface,
  e: RenderInput<"AbovePrompt">,
  withBrand = true,
) {
  const { checks, resolvedLine } = await read($, view);
  const { isDismissed, newCount, armedId } = await read($, strip);
  return renderStripView(
    $.ui.resolve(e),
    e,
    checks,
    resolvedLine,
    isDismissed,
    newCount,
    armedId,
    {
      act: (id) => act($, id),
      view: () => viewChecks($),
      dismiss: async () => {
        await update($, strip, (current) => ({
          ...current,
          isDismissed: true,
          newCount: 0,
        }));
      },
    },
    withBrand,
  );
}

async function viewChecks($: EngineInterface) {
  if (!checksEnabled()) return;
  await openPane($);
  const { checks } = await read($, view);
  const { openGroups } = await read($, pane);
  const id = visibleRowIds(
    checks.filter((check) => !check.isHidden),
    openGroups,
  )[0];
  if (id === undefined) return;
  try {
    await $.ui.focus({ requestId: PANE, key: `row-${id}` });
  } catch {
    // The focus is a help only. The pane is open without it.
  }
}

function openPane($: EngineInterface, isFocused = true) {
  if (!checksEnabled()) return;
  return $.ui.open({
    id: PANE,
    title: "antiburn",
    ...(isFocused ? { focus: true as const } : {}),
    columns: 48,
    rows: 24,
  });
}

// A new check that needs a fix opens the pane beside the prompt. The pane does not ask for the keys:
// a turn ends over an empty composer, so the host would give them, and the next prompt would go to
// the pane. View checks moves the focus. An open pane, or one the person closed, stays as it is.
async function showPane($: EngineInterface, view: LiveView): Promise<void> {
  if (isPaneClosedByPerson) return;
  const isNewActive = view.checks.some(
    (check) =>
      check.stage === "active" &&
      !check.isHidden &&
      view.newIds.includes(check.id),
  );
  if (!isNewActive) return;
  try {
    if ((await $.ui.panes()).some((open) => open.id === PANE)) return;
    await openPane($, false);
  } catch {
    // The pane is a help only. The strip and the chip show the check without it.
  }
}

/** Writes the tracker's view to state. Call it only at a turn boundary or after a press. */
async function publish($: EngineInterface): Promise<void> {
  if (!checksEnabled()) return;
  if (isDemo || tour === null) return;
  const next = tour.checks.takeView();
  const live = liveChecks(next);
  const resolvedLine = live.length === 0 ? next.resolvedNow : [];
  await update($, view, (current) => ({
    checks: next.checks,
    resolvedLine,
    entered: enteredStages(current.checks, next.checks),
  }));
  if (next.newIds.length > 0) {
    await update($, strip, (current) => ({
      isDismissed: false,
      newCount: current.isDismissed ? next.newIds.length : 0,
      armedId: null,
    }));
    await showPane($, next);
  }
  await showStatus($, next.checks);
}

// The status line carries the chip label, so it counts as the chip does.
async function showStatus(
  $: EngineInterface,
  checks: readonly LiveCheckView[],
  isCurrent: () => boolean = () => true,
): Promise<void> {
  if (!checksEnabled() || !isCurrent()) return;
  const surfaces = await $.session.surfaces();
  if (!checksEnabled() || !isCurrent()) return;
  const hasNoStrip = surfaces.some(
    (surface) => surface === "vscode" || surface === "mobile",
  );
  if (!hasNoStrip) return;
  const hasLive =
    liveChecks({ checks: [...checks], newIds: [], resolvedNow: [] }).length > 0;
  if (!checksEnabled() || !isCurrent()) return;
  $.ui.status(hasLive ? chipLabel(checks) : undefined);
}

/**
 * Does a demo fix as a real fix does: it sends the prompt, compacts the session, fills the prompt box or closes advice.
 * The live tracker has no demo checks, so the card moves to Awaiting verification here.
 */
async function demoAct(
  $: EngineInterface,
  check: LiveCheckView,
): Promise<void> {
  if (!checksEnabled()) return;
  await update($, strip, (current) => ({ ...current, armedId: null }));
  if (check.actionKind === "advise") {
    // Advice sends nothing. The card goes, as a live card goes quiet.
    const { checks } = await update($, view, (current) => ({
      ...current,
      checks: current.checks.map((one) =>
        one.id === check.id ? { ...one, isHidden: true } : one,
      ),
    }));
    await showStatus($, checks);
    return;
  }
  const { checks } = await update($, view, (current) => ({
    ...current,
    checks: current.checks.map((one) =>
      one.id === check.id ? { ...one, stage: "watching" as const } : one,
    ),
    entered: ["watching" as const],
  }));
  await showStatus($, checks);
  if (check.actionKind === "submit") {
    await $.prompt.submit({ text: check.prompt });
    return;
  }
  if (check.actionKind === "fill") {
    await $.prompt.fill({ text: check.prompt, mode: "replace" });
    return;
  }
  try {
    await $.session.compact({ instructions: check.prompt });
  } catch (error) {
    $.ui.log(
      "Compaction did not run. This session has too few messages to compact yet.",
    );
    $.ui.log(
      `compact failed: ${error instanceof Error ? error.message : String(error)}`,
      {
        to: "debug",
      },
    );
  }
}

async function act(
  $: EngineInterface,
  id: LiveCheckId,
  from: "card" | "other" = "other",
): Promise<void> {
  if (!checksEnabled()) return;
  const { checks } = await read($, view);
  const check = checks.find((one) => one.id === id);
  if (check === undefined) return;
  if (check.source === "session") return;
  const { armedId } = await read($, strip);
  if (check.actionKind === "advise") {
    await disarm($);
    if (isDemo) return demoAct($, check);
    shownTracker().acknowledge(id);
    await publish($);
    return;
  }
  // A card press compacts only on a second press: compaction cannot be undone, and the card shows no evidence.
  if (check.actionKind === "compact" && from === "card" && armedId !== id) {
    await update($, strip, (current) => ({ ...current, armedId: id }));
    await update($, pane, (current) => ({ ...current, selectedId: id }));
    return;
  }
  if (check.actionKind === "submit") {
    // In an unattended mode the first press shows the prompt and a second press sends it.
    if (isUnattendedMode() && armedId !== id) {
      await update($, strip, (current) => ({ ...current, armedId: id }));
      // The armed card opens, so the person reads the prompt before the second press.
      await update($, pane, (current) => ({ ...current, selectedId: id }));
      if (from === "card") return;
      await openPane($);
      return;
    }
    if (isDemo) return demoAct($, check);
    shownTracker().fixSent(id);
    await update($, strip, (current) => ({ ...current, armedId: null }));
    await publish($);
    await $.prompt.submit({ text: check.prompt });
    return;
  }
  if (isDemo) return demoAct($, check);
  if (check.actionKind === "compact") {
    await update($, strip, (current) => ({ ...current, armedId: null }));
    shownTracker().fixSent(id);
    await publish($);
    try {
      const compacted = await $.session.compact({ instructions: check.prompt });
      // The mod's own session.compact hook does not see this call, so reset here.
      if (compacted.skip === undefined && shownTracker().compact())
        await publish($);
    } catch (error) {
      // A short session has too few messages to compact. Keep the check active.
      shownTracker().unwatch(id);
      await publish($);
      $.ui.log(
        "Compaction did not run. This session has too few messages to compact yet.",
      );
      $.ui.log(
        `compact failed: ${error instanceof Error ? error.message : String(error)}`,
        {
          to: "debug",
        },
      );
    }
    return;
  }
  if (tour === null) {
    shownTracker().fillPressed(id);
  } else {
    // The real prompt hook feeds only the live tracker, so a tour fill goes to Awaiting verification at once.
    tour.checks.fixSent(id);
    await publish($);
  }
  await $.prompt.fill({ text: check.prompt, mode: "replace" });
}

// Another press or a new prompt cancels an armed fix, so a later press starts again from the first step.
async function disarm($: EngineInterface): Promise<void> {
  const { armedId } = await read($, strip);
  if (armedId !== null)
    await update($, strip, (current) => ({ ...current, armedId: null }));
}

/** Starts the demo tour on an empty tracker and opens the pane. */
async function startTour($: EngineInterface): Promise<void> {
  if (!checksEnabled()) return;
  invalidateSessionCheckReads();
  isDemo = false;
  tour = { checks: new LiveChecks(), beat: 0 };
  await update($, strip, () => ({
    isDismissed: false,
    newCount: 0,
    armedId: null,
  }));
  await update($, pane, (current) => ({ ...current, selectedId: null }));
  await publish($);
  await openPane($);
}

/** Plays the next tour step as one main turn. Returns its title, or null when no step is left. */
async function nextTourStep($: EngineInterface): Promise<string | null> {
  if (!checksEnabled()) return null;
  const beat = tour === null ? undefined : TOUR[tour.beat];
  if (tour === null || beat === undefined) return null;
  tour.beat += 1;
  await disarm($);
  // A dismissed strip counts the new checks, so the next publish shows "(N new)".
  if (beat.dismissesStrip)
    await update($, strip, (current) => ({
      ...current,
      isDismissed: true,
      newCount: 0,
    }));
  beat.run(tour.checks);
  tour.checks.turnEnd(undefined);
  await publish($);
  return beat.title;
}

/** Ends the sample view and the tour, and shows the live checks again. */
async function endDemo($: EngineInterface): Promise<void> {
  if (!checksEnabled()) return;
  invalidateSessionCheckReads();
  isDemo = false;
  tour = null;
  await update($, pane, (current) => ({ ...current, selectedId: null }));
  if (tour === null) await restoreSessionChecks($);
  else await publish($);
}

async function hide($: EngineInterface, id: LiveCheckId): Promise<void> {
  if (!checksEnabled()) return;
  await disarm($);
  const currentView = await read($, view);
  const target = currentView.checks.find((check) => check.id === id);
  if (target?.source === "session") {
    const next = {
      ...currentView,
      checks: currentView.checks.map((check) =>
        check.id === id ? { ...check, isHidden: true } : check,
      ),
    };
    sessionCheckCache = next;
    await update($, view, () => next);
    await showStatus($, next.checks);
    await update($, pane, (current) => ({ ...current, selectedId: null }));
    return;
  }
  if (isDemo) {
    await update($, view, (current) => ({
      ...current,
      checks: current.checks.map((check) =>
        check.id === id ? { ...check, isHidden: true } : check,
      ),
    }));
  }
  shownTracker().hide(id);
  await update($, pane, (current) => ({ ...current, selectedId: null }));
  await publish($);
}

/**
 * Active and watched checks show at once, in two sections: the app also lists checks that await verification apart.
 * Resolved checks fold, as the app folds Passed checks.
 * The list leaves out checks that are still building evidence or quiet: they need nothing from the user yet.
 */
// A desktop card opens in place. Only one card is open at a time.
async function toggleCard($: EngineInterface, id: LiveCheckId) {
  await pressInPane($, (current) => ({
    ...current,
    selectedId: current.selectedId === id ? null : id,
    turned: id,
  }));
}

// Each state call is a round trip to the host, and a busy host makes each one slow. So a press
// in the pane sends its three calls together, and the pane change does not wait for the others.
async function pressInPane(
  $: EngineInterface,
  change: (current: LivePaneState) => LivePaneState,
) {
  await Promise.all([disarm($), settleIcons($), update($, pane, change)]);
}

// A terminal card opens in place, as a desktop card does. The open card can be taller than
// the free rows, so the pane scrolls its top to the top of the window. With `nearest`, the
// engine can measure the closed card, find it already in view, and not move.
async function toggleRow($: EngineInterface, id: LiveCheckId) {
  await toggleCard($, id);
  const { selectedId } = await read($, pane);
  if (selectedId !== id) return;
  try {
    await $.ui.scroll({ to: { key: `item-${id}` }, in: PANE, block: "start" });
  } catch {
    // The pane does not draw the card yet, so it stays where it is.
  }
}

// The pane draws a section icon again on each render, and the desktop then plays its draw-in again.
// The draw-in is for a check that moves into a section, so a press in the pane stops it.
async function settleIcons($: EngineInterface) {
  // Most presses find no draw-in, so they read and do not write.
  const { entered } = await read($, view);
  if (entered.length > 0)
    await update($, view, (current) => ({ ...current, entered: [] }));
}

// A folded group opens and closes from its title and from its chevron.
async function toggleGroupOpen($: EngineInterface, title: string) {
  await pressInPane($, (current) => ({
    ...current,
    openGroups: toggleGroup(current.openGroups, title),
    turned: title,
  }));
}

// A card shows the first sentence as its reason. The open card shows the rest.
async function toggleFullPrompt($: EngineInterface, id: LiveCheckId) {
  await update($, pane, (current) => ({
    ...current,
    fullPromptId: current.fullPromptId === id ? null : id,
  }));
}

// The fix of an open card or a detail view. Advice and a fill fix with a command take one line.
// Other fixes show their prompt in a block.
// A Button takes only a text label and an Svg takes no press. So the chevron is an Svg,
// and a Button with a blank label lies on top of it to take the press.
// The Svg stays an image: the interactive frame draws an opaque white box on the desktop.
// An Svg image still runs its own CSS animation.
async function renderPane($: EngineInterface, e: PaneEvent) {
  const ui = $.ui.resolve(e);
  const { checks, entered, connection, freshnessEvaluated } = await read($, view);
  const { selectedId, openGroups, turned, fullPromptId } = await read($, pane);
  const { armedId } = await read($, strip);
  return renderChecksPaneView({
    ui,
    event: e,
    checks,
    entered,
    selectedId,
    openGroups,
    turned,
    fullPromptId,
    armedId,
    tourBeat: tour?.beat ?? null,
    connectionText: isDemo || tour !== null
      ? undefined
      : sessionConnectionText(connection, freshnessEvaluated),
    actions: {
      centredLine: (event, key, line) => centredLineView(ui, event, key, line),
      toggleGroup: (title) => toggleGroupOpen($, title),
      toggleCard: (id) => toggleCard($, id),
      toggleRow: (id) => toggleRow($, id),
      act: (id, from) => act($, id, from),
      hide: (id) => hide($, id),
      toggleFullPrompt: (id) => toggleFullPrompt($, id),
      nextTourStep: () => nextTourStep($),
      endTour: () => endDemo($),
    },
  });
}

// Each gating hook fails open: its .catch runs the rest of the chain, so a
// failure in this mod never blocks a tool call, a command or a turn.
type DevRegisterOptions = {
  usageUiEnabled?: boolean;
  startupNoticeEnabled?: boolean;
};

async function detectReleaseUsage($: EngineInterface): Promise<boolean> {
  try {
    const description = await $.antiburn.describe();
    return (
      description?.id === "antiburn" &&
      description.capabilities?.usageUiEnabled === true
    );
  } catch {
    return false;
  }
}

async function resolveReleaseUsage($: EngineInterface): Promise<boolean> {
  if (releaseOwnershipResolved) return releaseOwnsUsage;
  releaseOwnsUsage = await detectReleaseUsage($);
  releaseOwnershipResolved = true;
  return releaseOwnsUsage;
}

async function checkCli($: EngineInterface) {
  try {
    const result = await $.process.run(
      ["node", `${$.plugin.root}/lib/probe.mjs`],
      {
        timeoutMs: 2500,
      },
    );
    if (result.exitCode !== 0 || result.stdout.length > 256)
      return { state: "unavailable" as const };
    const value = JSON.parse(result.stdout);
    return {
      state: ["found", "not-found", "unavailable"].includes(value?.state)
        ? (value.state as "found" | "not-found" | "unavailable")
        : ("unavailable" as const),
    };
  } catch {
    return { state: "unavailable" as const };
  }
}

async function usageReply(
  $: EngineInterface,
  explicitRead = true,
): Promise<string> {
  if (explicitRead) await readUsage($, true);
  const state = await read($, usage);
  const now = await $.clock.now();
  if (!state.snapshot.windows.some((window) => window.percent !== null))
    return state.snapshot.status === "error"
      ? "Usage is unavailable."
      : "Usage has not been reported yet.";
  return PRIMARY_WINDOWS.map((kind) =>
    meterDescription(primaryWindow(state.snapshot, kind), now),
  ).join("\n");
}

export const register: Register = (on, options: DevRegisterOptions = {}) => {
  usageUiEnabled = options.usageUiEnabled !== false;
  startupNoticeEnabled = options.startupNoticeEnabled === true;
  on("engine.create", async ($, e, next) => {
    const built = await next(e);
    return {
      ...built,
      antiburnDevelopment: {
        describe: async () => ({
          id: "antiburn-dev",
          capabilities: { liveChecks: true },
        }),
      },
    };
  });

  on("session.start", async ($, e, next) => {
    releaseOwnsUsage = await detectReleaseUsage($);
    releaseOwnershipResolved = true;
    cancelUsageSequence();
    resetBurn();
    await update($, burn, () => ({
      active: false,
      previewUntil: 0,
      reduced: false,
      startedAt: 0,
      coolingAt: null,
      revision: 0,
      estimatedTps: 0,
      measuredTps: null,
      source: "unknown",
    }));
    usagePreviewRevision += 1;
    cancelUsagePresentation();
    usageMount = undefined;
    previousUsage = undefined;
    let sessionId: string | undefined;
    try {
      sessionId = await $.session.id();
    } catch {
      sessionId = undefined;
    }
    usageLayoutVariant = "inline";
    await update($, usage, (current) => ({
      ...current,
      sequence: undefined,
      layout: "inline" as const,
    }));
    const priorUsage = await read($, usage);
    if (
      priorUsage.preview &&
      usageFixture(priorUsage.preview.id, priorUsage.preview.capturedAt) ===
        undefined
    ) {
      await update($, usage, (current) => ({
        ...current,
        preview: null,
        previewReveal: undefined,
      }));
    }
    usageEntrance.load(
      sessionId !== undefined && priorUsage.presentationSessionId !== sessionId,
    );
    if (sessionId !== undefined)
      await update($, usage, (current) => ({
        ...current,
        presentationSessionId: sessionId,
      }));
    mode = liveMode(await $.env.get("ANTIBURN_LIVE_MODE"));
    invalidateSessionCheckReads();
    await clearDisabledChecks($);
    if ((await $.ui.panes()).some((open) => open.id === "usage"))
      await $.ui.close({ id: "usage" });
    usageOrder.reset();
    needsUsageReconciliation = false;
    resetTracker();
    if (checksEnabled()) await resetSessionChecks($, sessionId);
    else {
      sessionCheckSessionId = sessionId;
      sessionCheckCache = undefined;
    }
    if (usageUiEnabled && !releaseOwnsUsage) await readUsage($);
    await publish($);
    const registered = await $.command.register({
      name: "antiburn-dev",
      description: "Show antiburn help, usage, or live checks",
      argumentHint:
        "[help|status|diagnostics|usage [details|refresh]|checks|demo]",
    });
    commandName = registered.command;
    $.ui.log(`antiburn-live: command registered as /${commandName}`, {
      to: "debug",
    });
    const started = await next(e);
    if (
      startupNoticeEnabled &&
      sessionId !== undefined &&
      startupNoticeSessionId !== sessionId
    ) {
      startupNoticeSessionId = sessionId;
      const availability = await checkCli($);
      $.ui.log(renderNotice(availability));
    }
    await refreshSessionChecks($, sessionId);
    return started;
  }).catch(($, e, next) => next(e));

  on("classic.SessionStart", async ($, e, next) => {
    if (e.source === "clear") {
      cancelUsageSequence();
      resetBurn();
      await resetBurnState($);
      await update($, usage, (current) => ({
        ...current,
        sequence: undefined,
      }));
      usagePreviewRevision += 1;
      cancelUsagePresentation();
      usageOrder.reset();
      resetTracker();
      needsUsageReconciliation = true;
      await reconcileUsage($);
    }
    return next(e);
  }).catch(($, e, next) => next(e));

  on("session.end", async ($, e, next) => {
    hoverSites.clear();
    cancelUsageSequence();
    resetBurn();
    await resetBurnState($);
    await update($, usage, (current) => ({ ...current, sequence: undefined }));
    usagePreviewRevision += 1;
    cancelUsagePresentation();
    usageOrder.reset();
    if (e.reason === "clear" || e.reason === "resume") {
      resetTracker();
      needsUsageReconciliation = true;
    }
    return next(e);
  }).catch(($, e, next) => next(e));

  on("session.detach", async ($, e, next) => {
    hoverSites.clear();
    resetTerminalFlames();
    const result = await next(e);
    try {
      const surfaces = await $.session.surfaces();
      for (const site of burnSites) {
        if (!surfaces.some((surface) => site.startsWith(`${surface}:`)))
          burnSites.delete(site);
      }
      burnVisible = burnSites.size > 0;
    } catch {
      /* Finite playback expires without roster data. */
    }
    return result;
  });

  on("classic.UserPromptSubmit", async ($, e, next) => {
    if (!checksEnabled()) return next(e);
    noteMode(e.permission_mode);
    await disarm($);
    return next(e);
  }).catch(($, e, next) => next(e));

  on("classic.Stop", ($, e, next) => {
    if (!checksEnabled()) return next(e);
    noteMode(e.permission_mode);
    return next(e);
  }).catch(($, e, next) => next(e));

  on("prompt.submit", async ($, e, next) => {
    await reconcileUsage($);
    return next(e);
  }).catch(($, e, next) => next(e));

  on("command.run", async ($, e, next) => {
    if (e.command === "clear" || e.command === "resume") {
      const result = await next(e);
      await reconcileUsage($);
      return result;
    }
    if (e.command !== commandName) return next(e);
    const args = e.args.trim();
    if (args === "") {
      if (!checksEnabled())
        return { text: "Usage limits are shown above the prompt." };
      await openPane($);
      return { text: "Opened the antiburn checks pane." };
    }
    if (args === "checks") {
      if (!checksEnabled()) return { text: `Usage: /${commandName}` };
      await openPane($);
      return { text: "Opened the antiburn checks pane." };
    }
    if (args === "debug") {
      await $.ui.open({
        id: DEBUG_PANE,
        title: "antiburn debug",
        focus: true,
        columns: 64,
        rows: 24,
      });
      return { text: "Opened the antiburn debug pane." };
    }
    if (args === "demo tour") {
      if (!checksEnabled()) return { text: `Usage: /${commandName}` };
      await startTour($);
      return {
        text: `Started the demo tour (${TOUR.length} steps). Run /${commandName} demo next or press Next step in the pane for each step.`,
      };
    }
    if (args === "demo next") {
      if (!checksEnabled()) return { text: `Usage: /${commandName}` };
      if (tour === null)
        return { text: `Start the tour first with /${commandName} demo tour.` };
      const title = await nextTourStep($);
      if (title === null)
        return {
          text: `The tour has no more steps. Run /${commandName} demo off to go back to the live checks.`,
        };
      return { text: `Tour step ${tour.beat} of ${TOUR.length}: ${title}.` };
    }
    if (args === "demo" || args === "demo states") {
      if (!checksEnabled()) return { text: `Usage: /${commandName}` };
      await showCheckDemo($);
      return {
        text: "Showing sample checks. Run /antiburn-dev demo off to go back to the live checks.",
      };
    }
    if (args === "demo off") {
      if (!checksEnabled()) return { text: `Usage: /${commandName}` };
      await endDemo($);
      return { text: "Showing the live checks again." };
    }

    const parsed = parseCommand(args);
    const meta = describe(
      "claude-code",
      `/${commandName}`,
      "native",
      "unknown",
    );
    if (parsed.command === "help")
      return { text: renderHelp(meta, parsed.style) };
    if (parsed.command === "status" || parsed.command === "diagnostics") {
      const availability = await checkCli($);
      const status = renderStatus(meta, parsed.style);
      return {
        text: [
          status,
          `CLI: ${availability.state === "found" ? "Found" : availability.state === "not-found" ? "Not found" : "Check unavailable"}`,
          "App connection: Not checked",
          ...(parsed.command === "diagnostics"
            ? [
                `Checks pane: ${checksEnabled() ? "enabled" : "disabled"}; local detectors: disabled`,
              ]
            : []),
        ].join("\n"),
      };
    }
    if (parsed.command === "usage" || parsed.command === "details")
      return { text: await usageReply($) };
    if (parsed.command === "refresh") {
      await readUsage($, true);
      return { text: await usageReply($, false) };
    }
    return {
      text: `Usage: /${commandName} [help|status|diagnostics|usage [details|refresh]|checks|demo]`,
    };
  }).catch(($, e, next) => next(e));

  on("turn.step", async function* ($, e, next) {
    if (!BURN_ANIMATION_ENABLED) {
      const result = yield* next(e);
      return result;
    }
    const generation = burnGeneration;
    const id = `${generation}:${++burnSequence}`;
    let observed = false;
    let sampled = false;
    try {
      const now = await $.clock.now();
      if (generation === burnGeneration && burnPending.size < 64) {
        burnActivity.start(id, now);
        burnPending.set(id, 0);
        observed = true;
      }
    } catch {
      /* Missing measurements do not stop the stream. */
    }
    const stream = next(e);
    let finished = false;
    try {
      let item = await stream.next();
      while (!item.done) {
        const chunk = item.value;
        try {
          if (observed && generation === burnGeneration) {
            const count =
              chunk.kind === "text" || chunk.kind === "thinking"
                ? chunk.text.length
                : chunk.kind === "input"
                  ? chunk.json.length
                  : 0;
            if (count > 0) {
              burnPending.set(
                id,
                Math.min(1000000, (burnPending.get(id) ?? 0) + count),
              );
              if (!sampled || (burnPending.get(id) ?? 0) >= 128) {
                sampled = true;
                observeBurn($, generation);
              }
            }
          }
        } catch {
          /* Observation failures do not drop chunks. */
        }
        yield chunk;
        item = await stream.next();
      }
      finished = true;
      const result = item.value;
      if (observed && generation === burnGeneration) {
        try {
          await burnObservation?.catch(() => {});
          const now = await $.clock.now();
          if (generation === burnGeneration) {
            const count = burnPending.get(id) ?? 0;
            if (count > 0) burnActivity.chunk(id, count, now);
            burnActivity.complete(id, result.usage?.output_tokens ?? null, now);
            burnPending.delete(id);
            await tickBurn($, generation, true);
          }
        } catch {
          /* Missing measurements do not change the result. */
        }
      }
      return result;
    } finally {
      try {
        if (!finished) await stream.return(undefined as never);
      } finally {
        if (generation === burnGeneration) {
          burnPending.delete(id);
          if (observed && !finished) {
            try {
              await burnObservation?.catch(() => {});
              const now = await $.clock.now();
              if (generation === burnGeneration) {
                burnActivity.end(id, now);
                await tickBurn($, generation, true);
              }
            } catch {
              /* Cleanup stays local. */
            }
          }
        }
      }
    }
  });

  on("tool.call", async ($, e, next) => {
    return next(e);
  }).catch(($, e, next) => next(e));

  on("turn.complete", async ($, e, next) => {
    const result = await next(e);
    if (e.agentId !== undefined) return result;
    let sessionId: string | undefined;
    try {
      sessionId = await $.session.id();
    } catch {
      sessionId = undefined;
    }
    await refreshSessionChecks($, sessionId);
    return result;
  }).catch(($, e, next) => next(e));

  on("session.measure", async ($, e, next) => {
    needsUsageReconciliation = false;
    if (usageUiEnabled && !releaseOwnsUsage) {
      const ticket = usageOrder.acceptEvent();
      const now = await $.clock.now();
      await commitUsage($, normalizeUsage(e.rateLimits, now), ticket);
    }
    return next(e);
  }).catch(($, e, next) => next(e));

  on("session.compact", async ($, e, next) => {
    return next(e);
  }).catch(($, e, next) => next(e));

  on("ui.message", async ($, e, next) => {
    const match = /^usage-hover-(reset|pace)-(five_hour|seven_day)$/.exec(
      e.element,
    );
    if (
      e.surface !== "terminal" ||
      e.component !== "AbovePrompt" ||
      e.module !== "hooks/usage-target.tsx" ||
      !match
    )
      return next(e);
    const data = e.data as {
      hovered?: unknown;
      activate?: unknown;
      binding?: unknown;
    } | null;
    if (
      !data ||
      typeof data.hovered !== "boolean" ||
      typeof data.activate !== "boolean" ||
      typeof data.binding !== "string" ||
      data.binding !== hoverSites.get(e.requestId)?.binding
    )
      return next(e);
    const target = match[1] === "pace" ? `pace-${match[2]}` : match[2]!;
    await update($, usage, (current) => {
      const hovered = data.hovered
        ? { target, requestId: e.requestId, binding: data.binding as string }
        : current.hovered?.target === target &&
            current.hovered.requestId === e.requestId
          ? null
          : current.hovered;
      if (!data.activate) return { ...current, hovered };
      const dismissed = current.activated === target;
      return {
        ...current,
        hovered,
        revealed: dismissed ? null : target,
        activated: dismissed ? null : target,
      };
    });
    return next(e);
  }).catch(($, e, next) => next(e));

  on("ui.close", async ($, e, next) => {
    hoverSites.delete(e.id);
    await update($, usage, (current) =>
      current.hovered?.requestId === e.id
        ? { ...current, hovered: null }
        : current,
    );
    stopTerminalFlame(e.id);
    stopTerminalPulse(e.id);
    if (checksEnabled() && e.id === PANE && e.origin.kind === "person")
      isPaneClosedByPerson = true;
    return next(e);
  }).catch(($, e, next) => next(e));

  on("ui.render", { component: "AbovePrompt" }, async ($, e, next) => {
    await resolveReleaseUsage($);
    const site = `${e.surface}:${e.requestId}`;
    if (BURN_ANIMATION_ENABLED && !e.props.hasSurvey && e.props.maxRows > 0)
      burnSites.add(site);
    else burnSites.delete(site);
    burnVisible = burnSites.size > 0;
    if (e.props.hasSurvey || e.props.maxRows <= 0) {
      if (e.surface === "terminal") {
        terminalHoverBinding(e.requestId, "hidden");
        stopTerminalFlame(e.requestId);
        stopTerminalPulse(e.requestId);
      }
      if (usageEntrance.state.phase === "running" || usageRoll !== undefined)
        cancelUsagePresentation();
      return next(e);
    }
    if (releaseOwnsUsage || !usageUiEnabled) {
      const downstream = await next(e);
      if (!checksEnabled() || e.props.isWorking) return downstream;
      const checkStrip = await renderStrip($, e, false);
      if (!checkStrip) return downstream;
      const { Box } = $.ui.resolve(e);
      return (
        <Box key="checks-with-downstream" flexDirection="column" gap={1}>
          {checkStrip}
          {downstream}
        </Box>
      );
    }
    const ui = $.ui.resolve(e);
    const usageState = await read($, usage);
    const preview = usageState.preview;
    const checkStrip =
      checksEnabled() && !e.props.isWorking
        ? await renderStrip($, e, false)
        : null;
    return renderUsageComposition({
      ui,
      event: e,
      checkStrip,
      syntheticLabel:
        preview && usageState.showSyntheticLabels
          ? `Synthetic usage · ${preview.id}`
          : undefined,
      usage: await renderUsage($, e),
    });
  });

  on("ui.render", { component: "SessionMode" }, async ($, e, next) => {
    if (!checksEnabled()) return next(e);
    const { checks } = await read($, view);
    const chip = chipView(checks);
    return renderSessionModeView({
      ui: $.ui.resolve(e),
      event: e,
      label: chip.label,
      mark: chip.mark,
      openPane: async () => {
        await openPane($);
      },
    });
  });

  on("ui.render", { component: "Pane", requestId: PANE }, ($, e, next) =>
    checksEnabled() ? renderPane($, e) : next(e),
  );
  on("ui.render", { component: "Pane", requestId: DEBUG_PANE }, ($, e) =>
    renderUsageDebug($, e),
  );
};
