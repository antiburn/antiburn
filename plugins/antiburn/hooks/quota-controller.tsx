import { atom, read, update } from "claude-code"
import {
  parseUsageCommand,
  renderUsageHelp,
} from "../lib/product.mjs"
import type { EngineInterface, On, Register, RenderInput, SessionAuthorization } from "claude-code"
import type { UsageState } from "../types"
import { APP_URL } from "./brand"
import { retryAfterTime, type AccountUsageResult } from "./account-usage"
import {
  filledDotCount,
  normalizeUsage,
  pace,
  paceLabel,
  percentage,
  primaryWindow,
  resetText,
  terminalDotCount,
  usageFailure,
  UsageOrder,
  PRIMARY_WINDOWS,
  type RateLimitReading,
  type UsageSnapshot,
  type UsageWindow,
} from "./usage"
import {
  BRAND_MARK_SVG,
  meterDescription,
  responsiveMeterSvg,
  RESPONSIVE_FRAME_WIDTH,
  usageStatusSvg,
  secondaryTextSvg,
} from "./usage-graphics"
import { renderUsageMeterBand } from "./usage-meter-band"
import { desktopPercentage, PRESENTATION, usageLayout } from "./usage-presentation"

import {
  ARGUMENT_HINT,
  COMMAND,
  describeCommands,
} from "./commands"

const BRAND = "#FF5914"
const usage = atom(
  { plugin: "antiburn", key: "usage" } as const,
  {
    snapshot: { status: "empty", windows: [], capturedAt: 0 },
    pendingRead: null,
    accountRetryAt: null,
    revealed: null,
    activated: null,
    hovered: null,
  } as UsageState,
)
const usageOrder = new UsageOrder()
const hoverSites = new Map<string, { shape: string; binding: string }>()
const hoverMount = Math.random().toString(36).slice(2)
let hoverRevision = 0
let uiEnabled = true
let uiGeneration = 0
const visibleSites = new Set<string>()
let manualUsageRead: Promise<void> | undefined
let needsUsageReconciliation = false
let desktopAttached = false
const newAccountState = () => ({
  snapshot: null as UsageSnapshot | null,
  lastResult: null as AccountUsageResult | null,
  nextAttempt: 0,
  failures: 0,
  epoch: 0,
})
let accountState = newAccountState()
let accountRequest:
  | {
      state: typeof accountState
      epoch: number
      promise: Promise<AccountUsageResult>
    }
  | undefined

export async function fetchAccountUsage($: EngineInterface): Promise<AccountUsageResult> {
  let authorization: SessionAuthorization
  try {
    authorization = await $.session.authorize()
  } catch {
    return { status: "unsupported" }
  }
  if (authorization?.kind !== "bearer") return { status: "unsupported" }
  try {
    const response = await $.http.fetch("https://api.anthropic.com/api/oauth/usage", {
      auth: authorization.handle,
      headers: {
        "anthropic-beta": "oauth-2025-04-20",
        accept: "application/json",
      },
    })
    if (!response.ok)
      return {
        status: "error",
        retryAfter: response.status === 429 ? response.headers["retry-after"] : undefined,
      }
    const payload: unknown = JSON.parse(response.text)
    if (!payload || typeof payload !== "object" || Array.isArray(payload))
      return { status: "error" }
    const readings: RateLimitReading[] = []
    for (const kind of ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet"]) {
      const value: unknown = (payload as Record<string, unknown>)[kind]
      if (!value || typeof value !== "object" || Array.isArray(value)) continue
      const window = value as Record<string, unknown>
      if (typeof window.utilization !== "number") continue
      readings.push({
        kind,
        percentUsed: window.utilization,
        resetsAt: typeof window.resets_at === "string" ? window.resets_at : undefined,
      })
    }
    const snapshot = normalizeUsage(readings, await $.clock.now())
    return snapshot.status === "ready" ? { status: "ready", snapshot } : { status: "error" }
  } catch {
    return { status: "error" }
  }
}

function terminalHoverBinding(requestId: string, shape: string): string {
  const previous = hoverSites.get(requestId)
  if (previous?.shape === shape) return previous.binding
  const binding = `${hoverMount}:${++hoverRevision}`
  if (hoverSites.size >= 8 && !hoverSites.has(requestId))
    hoverSites.delete(hoverSites.keys().next().value!)
  hoverSites.set(requestId, { shape, binding })
  return binding
}

async function commitUsage(
  $: EngineInterface,
  snapshot: UsageSnapshot,
  ticket: { generation: number; revision: number },
): Promise<void> {
  if (!usageOrder.isCurrent(ticket)) return
  await update($, usage, (current) =>
    usageOrder.isCurrent(ticket) ? { ...current, snapshot, accountRetryAt: null } : current,
  )
}

async function applyAccountResult(
  $: EngineInterface,
  result: AccountUsageResult | null,
  ticket: { generation: number; revision: number },
): Promise<void> {
  if (result?.status === "ready") {
    await commitUsage($, result.snapshot, ticket)
  } else if (result?.status === "error") {
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket)
        ? {
            ...current,
            snapshot: usageFailure(current.snapshot, current.snapshot.capturedAt),
            accountRetryAt: accountState.nextAttempt,
          }
        : current,
    )
  } else if (result?.status === "unsupported") {
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket)
        ? {
            ...current,
            accountRetryAt: null,
            snapshot: current.snapshot.windows.some((window) => window.percent !== null)
              ? usageFailure(current.snapshot, current.snapshot.capturedAt)
              : { status: "empty" as const, windows: [], capturedAt: 0 },
          }
        : current,
    )
  }
}

async function readAccountUsage(
  $: EngineInterface,
  now: number,
  ticket: { generation: number; revision: number },
): Promise<void> {
  const state = accountState
  const epoch = state.epoch
  while (accountRequest && (accountRequest.state !== state || accountRequest.epoch !== epoch)) {
    try {
      await accountRequest.promise
    } catch {
      // Continue this event after the previous request releases its slot.
    }
    if (state !== accountState || epoch !== state.epoch || !usageOrder.isCurrent(ticket)) return
  }
  if (!accountRequest && now < state.nextAttempt) {
    await applyAccountResult($, state.lastResult, ticket)
    return
  }
  if (!accountRequest) {
    const promise = (async () => {
      const result = await fetchAccountUsage($)
      const observedAt = await $.clock.now()
      if (state !== accountState || epoch !== state.epoch) return result
      state.lastResult = result
      if (result.status === "ready") {
        state.snapshot = result.snapshot
        state.failures = 0
        state.nextAttempt = observedAt + 60_000
      } else if (result.status === "error") {
        state.failures = Math.min(5, state.failures + 1)
        state.nextAttempt = Math.max(
          observedAt + Math.min(900_000, 60_000 * 2 ** (state.failures - 1)),
          retryAfterTime(result.retryAfter, observedAt),
        )
      } else {
        state.nextAttempt = observedAt + 60_000
      }
      return result
    })()
    const request = { state, epoch, promise }
    accountRequest = request
    void promise
      .finally(() => {
        if (accountRequest === request) accountRequest = undefined
      })
      .catch(() => {})
  }
  const result = await accountRequest.promise
  if (state !== accountState || epoch !== state.epoch || !usageOrder.isCurrent(ticket)) return
  await applyAccountResult($, result, ticket)
}

async function readUsage($: EngineInterface): Promise<void> {
  const ticket = usageOrder.beginRead()
  try {
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket) ? { ...current, pendingRead: ticket } : current,
    )
    if (!usageOrder.isCurrent(ticket)) return
    let native: UsageSnapshot | undefined
    let failed = false
    const now = await $.clock.now()
    try {
      native = normalizeUsage((await $.session.usage()).rateLimits, now)
    } catch {
      failed = true
    }
    if (!usageOrder.isCurrent(ticket)) return
    if (native?.status === "ready") {
      accountState.epoch++
      accountState.snapshot = null
      accountState.lastResult = null
      await commitUsage($, native, ticket)
    } else if (desktopAttached) {
      await readAccountUsage($, now, ticket)
    } else if (failed) {
      throw new Error("Usage unavailable")
    } else if (native) {
      await commitUsage($, native, ticket)
    }
  } catch {
    if (!usageOrder.isCurrent(ticket)) return
    await update($, usage, (current) =>
      usageOrder.isCurrent(ticket)
        ? {
            ...current,
            snapshot: usageFailure(current.snapshot, current.snapshot.capturedAt),
          }
        : current,
    )
  } finally {
    await update($, usage, (current) =>
      current.pendingRead?.generation === ticket.generation &&
      current.pendingRead.revision === ticket.revision
        ? { ...current, pendingRead: null }
        : current,
    )
  }
}

async function refreshUsage($: EngineInterface): Promise<void> {
  if (manualUsageRead) return manualUsageRead
  const request = readUsage($)
  manualUsageRead = request
  try {
    await request
  } finally {
    if (manualUsageRead === request) manualUsageRead = undefined
  }
}

function accountRetryText(retryAt: number | null, now: number): string | null {
  if (retryAt === null || !Number.isFinite(retryAt) || retryAt <= now) return null
  const date = new Date(Math.ceil(retryAt / 1000) * 1000)
  if (!Number.isFinite(date.getTime())) return "Account check is temporarily unavailable"
  try {
    const formatter = new Intl.DateTimeFormat("en-US", {
      hour: "numeric",
      minute: "2-digit",
      second: date.getUTCSeconds() === 0 ? undefined : "2-digit",
      hour12: true,
      timeZoneName: "short",
    })
    const timeZone = formatter.resolvedOptions().timeZone
    if (!timeZone) throw new Error("Time zone unavailable")
    const parts = formatter.formatToParts(date)
    const part = (kind: Intl.DateTimeFormatPartTypes) =>
      parts.find((value) => value.type === kind)?.value ?? ""
    const calendar = new Intl.DateTimeFormat("en-CA", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    })
    const dateKey = (value: number) => {
      const fields = calendar.formatToParts(value)
      return ["year", "month", "day"]
        .map((kind) => fields.find((field) => field.type === kind)?.value ?? "")
        .join("-")
    }
    const day = dateKey(date.getTime()) === dateKey(now) ? "" : `${dateKey(date.getTime())} `
    const seconds = part("second") ? `:${part("second")}` : ""
    return `Account retry at ${day}${part("hour")}:${part("minute")}${seconds}${part("dayPeriod").toLowerCase()} ${part("timeZoneName")}`
  } catch {
    const timestamp = date.toISOString()
    const day =
      timestamp.slice(0, 10) === new Date(now).toISOString().slice(0, 10)
        ? ""
        : `${timestamp.slice(0, 10)} `
    const hour = date.getUTCHours()
    const seconds = date.getUTCSeconds() === 0 ? "" : `:${timestamp.slice(17, 19)}`
    return `Account retry at ${day}${hour % 12 || 12}:${timestamp.slice(14, 16)}${seconds}${hour < 12 ? "am" : "pm"} UTC`
  }
}

const RETRY_EXPLANATION = "Refresh checks session."

async function resetUsage($: EngineInterface): Promise<void> {
  uiGeneration++
  visibleSites.clear()
  usageOrder.reset()
  manualUsageRead = undefined
  accountState = newAccountState()
  hoverSites.clear()
  await update($, usage, () => ({
    snapshot: { status: "empty" as const, windows: [], capturedAt: 0 },
    pendingRead: null,
    accountRetryAt: null,
    revealed: null,
    activated: null,
    hovered: null,
  }))
}

async function reconcileUsage($: EngineInterface): Promise<void> {
  if (!needsUsageReconciliation) return
  needsUsageReconciliation = false
  await readUsage($)
}

async function revealUsage($: EngineInterface, kind: string): Promise<void> {
  await update($, usage, (current) => {
    const dismissed = current.activated === kind
    return {
      ...current,
      revealed: dismissed ? null : kind,
      activated: dismissed ? null : kind,
    }
  })
}

async function openTerminalWebsite($: EngineInterface): Promise<void> {
  try {
    const windows = (await $.env.get("OS")) === "Windows_NT"
    if (windows) {
      const result = await $.process.run(["cmd.exe", "/c", "start", "", "https://antiburn.com"], {
        timeoutMs: 5000,
      })
      if (result.exitCode === 0) return
    } else {
      try {
        const result = await $.process.run(["open", "https://antiburn.com"], {
          timeoutMs: 5000,
        })
        if (result.exitCode === 0) return
      } catch {
        // Try the Linux opener if the macOS opener fails.
      }
      const result = await $.process.run(["xdg-open", "https://antiburn.com"], {
        timeoutMs: 5000,
      })
      if (result.exitCode === 0) return
    }
  } catch {
    // Show the address if the host cannot open the browser.
  }
  await $.ui.status(`Open ${APP_URL} in your browser`)
}

async function renderUsage($: EngineInterface, e: RenderInput<"AbovePrompt">) {
  const generation = uiGeneration
  if (visibleSites.size >= 8 && !visibleSites.has(e.requestId))
    visibleSites.delete(visibleSites.values().next().value!)
  visibleSites.add(e.requestId)
  const activate = (action: () => Promise<unknown>) => {
    if (uiEnabled && generation === uiGeneration && visibleSites.has(e.requestId)) return action()
  }
  const ui = $.ui.resolve(e)
  const { Box, Text, Button, Link } = ui
  const state = await read($, usage)
  const now = await $.clock.now()
  const snapshot = state.snapshot
  const windows = PRIMARY_WINDOWS.map((kind) => primaryWindow(snapshot, kind))
  const columns = Math.max(1, Math.min(4096, e.props.bodyColumns))
  const isDesktop = e.surface === "desktop" && "Svg" in ui
  const mark = isDesktop ? (
    <Box
      key="usage-mark-slot"
      alignItems="center"
      position="relative"
      width={PRESENTATION.markSlotSize / 8}
      flexShrink={0}
    >
      <ui.Svg
        key="usage-mark-size"
        source={BRAND_MARK_SVG}
        alt="antiburn"
        width={PRESENTATION.markSize}
        height={PRESENTATION.markSize}
      />
      <Box
        key="usage-brand-link"
        position="absolute"
        left={0}
        top={0}
        width={14 / 8}
        height={14 / 16}
        overflow="hidden"
      >
        <Link href={APP_URL} label="Open antiburn website">
          <Text>{"\u2003"}</Text>
        </Link>
      </Box>
    </Box>
  ) : (
    <Box
      key="usage-brand-text-slot"
      width={11}
      paddingLeft={1}
      flexDirection="row"
      flexShrink={0}
      height={2}
      alignItems="center"
      justifyContent="center"
    >
      <Box flexDirection="column" width={10} height={2} flexShrink={0}>
        <Text> </Text>
        <Box width={10} height={1} flexShrink={0} overflow="hidden">
          <Button
            key="usage-brand-open"
            plain
            label="Open antiburn website"
            onPress={() => activate(() => openTerminalWebsite($))}
          >
            <Text color={BRAND} backgroundColor="#4D2F23">
              {" antiburn "}
            </Text>
          </Button>
        </Box>
      </Box>
    </Box>
  )
  const hasReading = windows.some((window) => window.percent !== null)
  if (!hasReading && snapshot.status !== "ready") {
    terminalHoverBinding(e.requestId, "empty")
    const pending = state.pendingRead != null && usageOrder.isCurrent(state.pendingRead)
    const status = pending ? "Checking usage..." : "Usage unavailable"
    const canRefresh = !pending && columns >= 24
    const retryAt = state.accountRetryAt
    const retryHint = accountRetryText(retryAt, now)
    const graphic = usageStatusSvg(status, undefined, !pending && columns < 60)
    const band = (
      <Box
        key="usage-band"
        flexDirection="row"
        alignItems={isDesktop ? "center" : "flex-end"}
        gap={isDesktop ? 0 : 1}
        height={isDesktop && columns >= 24 ? graphic.height / 16 : undefined}
      >
        {isDesktop && <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />}
        {mark}
        {isDesktop && <Box width={PRESENTATION.brandGap / 8} flexShrink={0} />}
        {isDesktop ? (
          <ui.Svg
            key="usage-status"
            source={graphic.source}
            alt={graphic.description}
            width={Math.min(
              graphic.width,
              Math.max(
                1,
                columns * 8 -
                  PRESENTATION.brandInset -
                  PRESENTATION.markSlotSize -
                  PRESENTATION.brandGap -
                  (canRefresh ? 32 : 0),
              ),
            )}
            height={graphic.height}
          />
        ) : (
          <Box width={Math.max(1, columns - (canRefresh ? 14 : 12))} flexShrink={1}>
            {pending && e.surface === "terminal" && "Client" in ui ? (
              <ui.Client key="usage-checking" module="./checking-status.ts" props={null} />
            ) : (
              <Text key="usage-status" wrap="truncate-end">
                {status}
              </Text>
            )}
          </Box>
        )}
        {canRefresh && (
          <Button
            key="usage-refresh"
            plain
            dimColor
            label={isDesktop ? "↻" : "[↻]"}
            onPress={() => activate(() => refreshUsage($))}
          >
            <Text dimColor>{isDesktop ? "↻" : "[↻]"}</Text>
          </Button>
        )}
      </Box>
    )
    if (!retryHint) return band
    const retryCopy = `${retryHint} · ${RETRY_EXPLANATION}`
    const secondary = secondaryTextSvg(
      [retryCopy],
      columns * 8,
      Math.max(2, Math.floor(e.props.maxRows - 2)),
    )
    return (
      <Box key="usage-band-with-retry" flexDirection="column" width={columns}>
        {band}
        {isDesktop ? (
          <ui.Svg
            source={secondary.source}
            alt={secondary.description}
            width={secondary.width}
            height={secondary.height}
          />
        ) : (
          <Box key="usage-retry-copy" width={columns} overflow="hidden">
            <Text key="usage-account-retry" dimColor wrap="wrap">
              {retryCopy}
            </Text>
          </Box>
        )}
      </Box>
    )
  }

  if (isDesktop && columns >= 24) {
    terminalHoverBinding(e.requestId, "desktop")
    return renderUsageMeterBand({
      ui,
      layout: usageLayout(columns * 8, "inline"),
      windows,
      mark,
      renderMeter: (window) => (
        <ui.Svg
          key={`meter-graphic-${window.kind}`}
          source={responsiveMeterSvg(window, now)}
          alt={`${meterDescription(window, now, true)}${snapshot.status === "error" ? ". Usage unavailable · Last known" : ""}`}
          width={RESPONSIVE_FRAME_WIDTH}
          height={18}
          isInteractive
        />
      ),
    })
  }
  const content = Math.max(1, columns - (isDesktop ? 3 : 12))
  const inlineColumns = Math.max(1, Math.floor((content - 3) / 2))
  const isInline =
    columns >= 80 &&
    windows.every((window) => window.label.length + percentage(window).length + 2 <= inlineColumns)
  const meterColumns = isInline ? inlineColumns : content
  const geometry = (window: UsageWindow) => {
    const trackColumns = Math.max(
      0,
      meterColumns - window.label.length - percentage(window).length - 2,
    )
    const count = isDesktop ? 0 : terminalDotCount(trackColumns)
    const value = pace(window, now)
    const markerCell =
      value === null || count < 2
        ? null
        : Math.max(1, Math.min(count * 2 - 3, Math.round(value * (count - 1)) * 2 - 1))
    return { count, value, markerCell }
  }
  const hoverBinding =
    e.surface === "terminal"
      ? terminalHoverBinding(
          e.requestId,
          JSON.stringify([
            columns,
            snapshot.status,
            windows.map((window) => [
              window.resetsAt,
              percentage(window).length,
              geometry(window).markerCell,
            ]),
          ]),
        )
      : ""
  const revealed =
    e.surface === "terminal" &&
    state.hovered?.requestId === e.requestId &&
    state.hovered.binding === hoverBinding
      ? state.hovered.target
      : state.revealed

  const meter = (window: UsageWindow) => {
    const description = meterDescription(window, now, isDesktop)
    const isRevealed = revealed === window.kind
    const isPaceRevealed = revealed === `pace-${window.kind}`
    const { count, value, markerCell } = geometry(window)
    const isPaceReached = window.percent !== null && value !== null && window.percent / 100 >= value
    const cells = count * 2 - 1
    const copy = isRevealed
      ? resetText(window, now)
      : isPaceRevealed
        ? value === null
          ? "Pace unavailable"
          : paceLabel(value)
        : null
    const copyOnRight = markerCell !== null && cells - markerCell - 1 >= markerCell
    const copyOnLeft = isRevealed || (isPaceRevealed && !copyOnRight)
    const segment = (start: number, end: number, showCopy: boolean) => {
      const copyLength =
        showCopy && copy !== null ? Math.min(copy.length, Math.max(0, end - start)) : 0
      const copyStart = copyOnLeft ? end - copyLength : start
      const dotCells = Array.from({ length: Math.max(0, end - start) }, (_, index) => {
        const cell = start + index
        if (copyLength > 0 && cell >= copyStart && cell < copyStart + copyLength) return null
        if (cell % 2 !== 0) return <Text key={`space-${cell}`}> </Text>
        const filled = cell / 2 < filledDotCount(window.percent, count)
        return (
          <Text key={`dot-${cell}`} color={filled ? BRAND : "subtle"}>
            {"●"}
          </Text>
        )
      })
      if (copyLength === 0) return dotCells
      dotCells.splice(
        copyStart - start,
        copyLength,
        <Text key="usage-segment-copy" dimColor wrap="truncate-end">
          {copy}
        </Text>,
      )
      return dotCells
    }

    const target = (detail: "reset" | "pace", text: string, color?: string, dimColor?: boolean) =>
      e.surface === "terminal" && "Client" in ui ? (
        <ui.Client
          key={`usage-hover-${detail}-${window.kind}`}
          module="./usage-target.tsx"
          width={detail === "reset" ? window.label.length : 1}
          height={1}
          props={{
            binding: hoverBinding,
            width: detail === "reset" ? window.label.length : 1,
            text,
            ...(color === undefined ? {} : { color }),
            ...(dimColor === undefined ? {} : { dimColor }),
          }}
        />
      ) : null
    const markerGlyph = isPaceReached ? "│" : "┊"
    const markerHover = target(
      "pace",
      markerGlyph,
      isPaceReached ? BRAND : "subtle",
      !isPaceReached && !isPaceRevealed,
    )
    const paceMarker =
      markerCell !== null ? (
        <Box position="relative" width={1} height={1} flexShrink={0}>
          <Button
            key={`usage-pace-${window.kind}`}
            plain
            onPress={() => activate(() => revealUsage($, `pace-${window.kind}`))}
          >
            <Text
              key="pace-marker"
              color={isPaceReached ? BRAND : "subtle"}
              dimColor={!isPaceReached && !isPaceRevealed}
            >
              {isPaceReached ? "│" : "┊"}
            </Text>
          </Button>
          {markerHover && (
            <Box position="absolute" left={0} top={0} width={1} height={1}>
              {markerHover}
            </Box>
          )}
        </Box>
      ) : null
    const dots =
      count >= 2 ? (
        isRevealed && markerCell !== null ? (
          <Box
            key="usage-reset-track"
            position="relative"
            width={cells}
            height={1}
            overflow="hidden"
          >
            <Text>{segment(0, cells, false)}</Text>
            <Box
              key="usage-pace-marker-slot"
              position="absolute"
              left={markerCell}
              top={0}
              width={1}
              height={1}
            >
              <Button
                key={`usage-pace-${window.kind}`}
                plain
                onPress={() => activate(() => revealUsage($, `pace-${window.kind}`))}
              >
                <Text color={isPaceReached ? BRAND : "subtle"}>{markerGlyph}</Text>
              </Button>
            </Box>
            <Box
              key="usage-reset-copy-track"
              position="absolute"
              left={0}
              top={0}
              width={cells}
              height={1}
              overflow="hidden"
            >
              <Text key="usage-reset-copy" dimColor wrap="truncate-end">
                {copy}
              </Text>
            </Box>
            {markerHover && (
              <Box position="absolute" left={markerCell} top={0} width={1} height={1}>
                {target(
                  "pace",
                  copy !== null && markerCell < copy.length ? copy[markerCell]! : markerGlyph,
                  copy !== null && markerCell < copy.length
                    ? undefined
                    : isPaceReached
                      ? BRAND
                      : "subtle",
                  copy !== null && markerCell < copy.length ? true : !isPaceReached,
                )}
              </Box>
            )}
          </Box>
        ) : (
          <Box flexDirection="row" width={cells} height={1} flexShrink={0}>
            {markerCell === null && <Text>{segment(0, cells, copy !== null)}</Text>}
            {markerCell !== null && (
              <Box
                key={`usage-pace-left-${window.kind}`}
                width={markerCell}
                flexShrink={0}
                justifyContent={isPaceRevealed && copyOnLeft ? "flex-end" : undefined}
                height={1}
                flexDirection="row"
                overflow="hidden"
              >
                <Text wrap="truncate-end">
                  {segment(0, markerCell, copy !== null && copyOnLeft)}
                </Text>
              </Box>
            )}
            {paceMarker}
            {markerCell !== null && (
              <Box
                key={`usage-pace-right-${window.kind}`}
                width={cells - markerCell - 1}
                flexShrink={0}
                height={1}
                flexDirection="row"
                overflow="hidden"
              >
                <Text wrap="truncate-end">
                  {segment(markerCell + 1, cells, copy !== null && isPaceRevealed && copyOnRight)}
                </Text>
              </Box>
            )}
          </Box>
        )
      ) : null
    return (
      <Box
        key={`usage-${window.kind}`}
        position="relative"
        flexDirection="column"
        width={meterColumns}
        height={1}
      >
        <Box flexDirection="row" justifyContent="space-between" gap={1}>
          <Box
            position="relative"
            width={window.label.length}
            height={1}
            flexShrink={0}
            overflow="hidden"
          >
            <Button
              key={`usage-detail-${window.kind}`}
              plain
              label={description}
              onPress={() => activate(() => revealUsage($, window.kind))}
            >
              <Text>{window.label}</Text>
            </Button>
            {e.surface === "terminal" && (
              <Box position="absolute" left={0} top={0} width={window.label.length} height={1}>
                {target("reset", window.label)}
              </Box>
            )}
          </Box>
          <Box flexGrow={1} flexShrink={1} minWidth={0}>
            {dots}
          </Box>
          <Text>{isDesktop ? desktopPercentage(window) : percentage(window)}</Text>
        </Box>
      </Box>
    )
  }

  const meters = (
    <Box key="usage-meters" flexDirection={isInline ? "row" : "column"} gap={isInline ? 3 : 0}>
      {windows.map(meter)}
    </Box>
  )
  if (snapshot.status === "error") {
    return (
      <Box key="usage-band" flexDirection="column">
        <Box
          flexDirection="row"
          alignItems={isDesktop ? "center" : "flex-end"}
          gap={isDesktop ? 0 : 1}
        >
          {isDesktop && <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />}
          {mark}
          {isDesktop && <Box width={PRESENTATION.brandGap / 8} flexShrink={0} />}
          <Text key="usage-status">Usage unavailable · Last known</Text>
        </Box>
        <Box marginLeft={3}>{meters}</Box>
      </Box>
    )
  }
  return (
    <Box
      key="usage-band"
      position="relative"
      flexDirection="row"
      alignItems={isDesktop ? "center" : "flex-end"}
      gap={isDesktop ? 0 : 1}
    >
      {isDesktop && <Box width={PRESENTATION.brandInset / 8} flexShrink={0} />}
      {mark}
      {isDesktop && <Box width={PRESENTATION.brandGap / 8} flexShrink={0} />}
      {meters}
    </Box>
  )
}
export async function usageDetails(
  $: EngineInterface,
  commandName: string,
  confirmation?: string,
): Promise<{ text: string }> {
  const state = await read($, usage)
  const now = await $.clock.now()
  const retryHint = accountRetryText(state.accountRetryAt, now)
  if (!state.snapshot.windows.some((window) => window.percent !== null)) {
    return {
      text:
        state.pendingRead != null && usageOrder.isCurrent(state.pendingRead)
          ? "Checking usage…"
          : `Usage isn’t available right now. Run \`/${commandName} refresh\` to try again.${retryHint ? `\n${retryHint} · ${RETRY_EXPLANATION}` : ""}`,
    }
  }
  if (confirmation && state.snapshot.status === "ready" && state.pendingRead == null)
    return { text: confirmation }
  const prefix = state.snapshot.status === "error" ? "Usage unavailable · Last known.\n" : ""
  return {
    text:
      prefix +
      PRIMARY_WINDOWS.map((kind) =>
        meterDescription(primaryWindow(state.snapshot, kind), now),
      ).join("\n") +
      (retryHint ? `\n${retryHint} · ${RETRY_EXPLANATION}` : ""),
  }
}

export async function requestUsage($: EngineInterface): Promise<void> {
  await readUsage($)
}

export async function startQuota($: EngineInterface): Promise<void> {
  desktopAttached = false
  needsUsageReconciliation = false
  try {
    await resetUsage($)
    if (uiEnabled) await readUsage($)
  } catch {
    // Keep the host session usable when quota setup fails.
  }
}

export async function endQuota($: EngineInterface, reason: string): Promise<void> {
  try {
    await resetUsage($)
  } catch {
    // End the host session even if display cleanup fails.
  }
  needsUsageReconciliation = reason === "clear" || reason === "resume"
}

export async function reconcileQuota($: EngineInterface): Promise<void> {
  try {
    if (uiEnabled) await reconcileUsage($)
  } catch {
    // Preserve the host command result when quota refresh fails.
  }
}

export function registerCommands(on: On, usageUiEnabled: boolean) {
  let registered = false
  let generation = 0

  on("session.start", async ($, e, next) => {
    registered = false
    const current = ++generation
    await startQuota($)
    const result = await next(e)
    if (current !== generation) return result
    try {
      const command = await $.command.register({
        name: COMMAND,
        description: "Show your usage or refresh it",
        argumentHint: ARGUMENT_HINT,
      })
      if (current !== generation) return result
      registered = command.command === COMMAND
    } catch {
      if (current !== generation) return result
    }
    if (!registered) {
      try {
        await $.ui.log("antiburn: /antiburn could not be registered.", {
          to: "debug",
        })
      } catch {
        // Keep quota available when the host cannot log the registration failure.
      }
    }
    return result
  })

  on("session.end", async ($, e, next) => {
    await endQuota($, e.reason)
    if (e.reason !== "clear" && e.reason !== "resume") {
      generation++
      registered = false
    }
    return next(e)
  })

  on("ui.render", { component: "CommandOutput" }, (_$, e, next) => {
    if (e.props.command !== COMMAND) return next(e)
    return next({
      ...e,
      props: { ...e.props, text: e.props.text.replace(/^antiburn: /, "") },
    })
  })

  on("command.run", async ($, e, next) => {
    if (e.command === "clear" || e.command === "resume") {
      const result = await next(e)
      await reconcileQuota($)
      return result
    }
    if (!registered || e.command !== COMMAND) return next(e)
    const command = parseUsageCommand(e.args)
    if (command === "invalid" || command === "help")
      return { text: renderUsageHelp(`/${COMMAND}`, command === "invalid") }
    try {
      if (command === "refresh" || !usageUiEnabled) await requestUsage($)
      const confirmation =
        command === "refresh"
          ? usageUiEnabled
            ? "Usage updated."
            : `Usage updated. Run \`/${COMMAND}\` to see it.`
          : undefined
      return await usageDetails($, COMMAND, confirmation)
    } catch {
      return {
        text: `Usage isn’t available right now. Run \`/${COMMAND} refresh\` to try again.`,
      }
    }
  })
}

export function registerQuota(on: On, options: { usageUiEnabled: boolean }) {
  uiEnabled = options.usageUiEnabled

  on("classic.SessionStart", async ($, e, next) => {
    if (e.source === "clear") {
      try {
        await resetUsage($)
        needsUsageReconciliation = true
        if (uiEnabled) await reconcileUsage($)
      } catch {
        // Keep the host clear operation available when quota refresh fails.
      }
    }
    return next(e)
  })

  on("session.attach", async ($, e, next) => {
    const state = accountState
    const result = await next(e)
    if (e.surface === "desktop" && state === accountState) {
      desktopAttached = true
      if (uiEnabled) await readUsage($)
    }
    return result
  })

  on("session.detach", async ($, e, next) => {
    uiGeneration++
    visibleSites.clear()
    usageOrder.reset()
    manualUsageRead = undefined
    const hadAccountReading = accountState.snapshot !== null
    accountState = newAccountState()
    if (e.surface === "desktop") desktopAttached = false
    hoverSites.clear()
    try {
      await update($, usage, (current) => ({
        ...current,
        pendingRead: null,
        accountRetryAt: null,
        snapshot: hadAccountReading
          ? { status: "empty" as const, windows: [], capturedAt: 0 }
          : current.snapshot,
        hovered: null,
      }))
    } catch {
      // Detach the client even if hover cleanup fails.
    }
    return next(e)
  })

  on("session.measure", async ($, e, next) => {
    if (!uiEnabled) return next(e)
    if (desktopAttached && normalizeUsage(e.rateLimits, 0).status !== "ready") return next(e)
    const ticket = usageOrder.acceptEvent()
    needsUsageReconciliation = false
    try {
      const snapshot = normalizeUsage(e.rateLimits, await $.clock.now())
      if (!usageOrder.isCurrent(ticket)) return next(e)
      accountState.epoch++
      if (snapshot.status === "ready") {
        accountState.snapshot = null
        accountState.lastResult = null
      }
      if (snapshot.status === "ready" || accountState.snapshot === null)
        await commitUsage($, snapshot, ticket)
    } catch {
      try {
        await update($, usage, (current) =>
          usageOrder.isCurrent(ticket)
            ? {
                ...current,
                snapshot: usageFailure(current.snapshot, current.snapshot.capturedAt),
              }
            : current,
        )
      } catch {
        // Forward the measurement even if its unavailable label cannot be saved.
      }
    }
    return next(e)
  })

  on("ui.message", async ($, e, next) => {
    if (!uiEnabled) return next(e)
    const match = /^usage-hover-(reset|pace)-(five_hour|seven_day)$/.exec(e.element)
    if (
      e.surface !== "terminal" ||
      e.component !== "AbovePrompt" ||
      e.module !== "hooks/usage-target.tsx" ||
      !match
    )
      return next(e)
    const data = e.data as {
      hovered?: unknown
      activate?: unknown
      binding?: unknown
    } | null
    if (
      !data ||
      typeof data.hovered !== "boolean" ||
      typeof data.activate !== "boolean" ||
      typeof data.binding !== "string" ||
      data.binding !== hoverSites.get(e.requestId)?.binding
    )
      return next(e)
    const target = match[1] === "pace" ? `pace-${match[2]}` : match[2]!
    try {
      await update($, usage, (current) => {
        const hovered = data.hovered
          ? { target, requestId: e.requestId, binding: data.binding as string }
          : current.hovered?.target === target && current.hovered.requestId === e.requestId
            ? null
            : current.hovered
        if (!data.activate) return { ...current, hovered }
        const dismissed = current.activated === target
        return {
          ...current,
          hovered,
          revealed: dismissed ? null : target,
          activated: dismissed ? null : target,
        }
      })
    } catch {
      // Keep host input available when display state cannot be saved.
    }
    return next(e)
  })

  on("ui.close", async ($, e, next) => {
    visibleSites.delete(e.id)
    hoverSites.delete(e.id)
    try {
      await update($, usage, (current) =>
        current.hovered?.requestId === e.id ? { ...current, hovered: null } : current,
      )
    } catch {
      // Close the host UI even if hover cleanup fails.
    }
    return next(e)
  })

  on("ui.render", { component: "AbovePrompt" }, async ($, e, next) => {
    if (!uiEnabled || e.props.hasSurvey || e.props.maxRows <= 0) {
      visibleSites.delete(e.requestId)
      terminalHoverBinding(e.requestId, "hidden")
      return next(e)
    }
    const downstream = await next(e)
    try {
      const { Box } = $.ui.resolve(e)
      return (
        <Box
          key="usage-spacing"
          flexDirection="column"
          marginTop={
            e.props.maxRows >= 8 && e.props.bodyColumns >= 24
              ? e.surface === "desktop"
                ? 0.25
                : 1
              : 0
          }
        >
          {downstream}
          {await renderUsage($, e)}
        </Box>
      )
    } catch {
      // Use the host drawing when quota rendering fails.
    }
    return downstream
  })
}

export const register: Register = (on) => {
  const usageUiEnabled = true
  registerCommands(on, usageUiEnabled)

  on("engine.create", async ($, e, next) => {
    const engine = await next(e)
    return {
      ...engine,
      antiburn: {
        describe: async () => {
          const description = describeCommands()
          return {
            ...description,
            capabilities: {
              ...description.capabilities,
              usage: true,
              usageUiEnabled,
            },
          }
        },
      },
    }
  })
  registerQuota(on, { usageUiEnabled })
}
