import { PushButton } from "../ui/PushButton"
import { useStableAccountNumbers } from "./useStableAccountNumbers"
import "./sidebar-usage.css"
import { enableLiveUsage, useAppSettings } from "../../views/settings/useAppSettings"
import { noteInteraction, openSettingsWindow, refreshLiveUsage } from "../../lib/ipc"
import { Gauge, Pin, RotateCw, X } from "lucide-react"
import { useRef, useState } from "react"
import {
  accountDisplayName,
  ProviderGroup,
  ProviderRadial,
  UnavailableGroup,
} from "./UsageLimitsBar"
import { Tooltip } from "../presentation/Tooltip"
import { SegmentedControl } from "../ui/SegmentedControl"
import { UsageRing } from "./UsageRing"
import { providerMark } from "./ProviderUsagePrimitives"
import type {
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
  SidebarUsagePin as UsagePin,
} from "../../lib/ipc"
import {
  liveDisplayableProviders,
  liveFailureIsSetup,
  liveProviderStatus,
  liveUnavailableProviders,
  liveWindowLabel,
  liveWindows,
  maxLiveUsedPercent,
  orderedLiveAccounts,
} from "../../lib/presentation/liveUsage"
import { ConnectionDialog } from "../../views/settings/ConnectionDialog"

function usageFocusTarget() {
  return (
    document.querySelector<HTMLButtonElement>("[data-usage-limits-trigger]") ??
    document.querySelector<HTMLButtonElement>(".responsive-navigation-bar button")
  )
}

type UsageAppearance = "dials" | "bars"
type RefreshOutcome = "refreshed" | "partial" | "checked"

function hasProviderRefreshError(summary: LiveUsageSummaryPayload) {
  return summary.errors.some(({ detail }) => !liveFailureIsSetup(detail))
}

const pinButtonClass =
  "flex size-10 shrink-0 items-center justify-center rounded-control text-label-secondary hover:bg-surface-hover focus-visible:bg-surface-hover aria-pressed:text-label"
const pinId = (pin: UsagePin) => JSON.stringify([pin.account, pin.window, pin.scope])
const matchesWindow = (pin: UsagePin, window: LiveUsageWindowPayload) =>
  pin.window === window.id && pin.scope === window.scopeModel
export function usageCandidates(live: LiveUsageSummaryPayload) {
  const accounts = orderedLiveAccounts(liveDisplayableProviders(live))
  return accounts.filter(
    ({ reading }) =>
      accounts.filter(
        ({ reading: other }) =>
          other.provider === reading.provider &&
          (reading.accountKey
            ? other.accountKey === reading.accountKey
            : !other.accountKey && other.sourceLabel === reading.sourceLabel),
      ).length === 1,
  )
}
function useAccountNames(accounts: ReturnType<typeof usageCandidates>) {
  const numbers = useStableAccountNumbers(
    accounts.map(({ key, reading }) => ({ key, provider: reading.provider })),
  )
  const counts = new Map<string, number>()
  for (const { reading } of accounts)
    counts.set(reading.provider, (counts.get(reading.provider) ?? 0) + 1)
  return new Map(
    accounts.map(({ key, reading }) => [
      key,
      accountDisplayName(reading, key, numbers, counts),
    ]),
  )
}
export function defaultUsagePins(live: LiveUsageSummaryPayload): UsagePin[] {
  const priority = (provider: string) =>
    provider === "anthropic" ? 0 : provider === "openai" ? 1 : 2
  const accounts = usageCandidates(live).sort(
    (a, b) => priority(a.reading.provider) - priority(b.reading.provider),
  )
  const pins: UsagePin[] = []
  const vendors = new Set<string>()
  for (const { key, reading } of accounts) {
    if (vendors.has(reading.provider)) continue
    const windows = liveWindows(reading)
    const weeklyDefault = reading.provider === "anthropic" || reading.provider === "openai"
    const window = weeklyDefault
      ? windows.find((entry) => entry.scopeModel === null && entry.kind === "weekly")
      : (windows.find((entry) => entry.usedPercent === maxLiveUsedPercent(reading)) ??
        windows[0])
    if (!window) continue
    pins.push({
      account: key,
      provider: reading.provider,
      name: reading.displayName,
      window: window.id,
      scope: window.scopeModel,
      label: liveWindowLabel(window),
    })
    vendors.add(reading.provider)
    if (pins.length === 3) break
  }
  return pins
}
export function SidebarUsage({
  live,
  settled = live !== null,
}: {
  live: LiveUsageSummaryPayload | null
  settled?: boolean
}) {
  const { settings, loaded, updateSidebar: update } = useAppSettings()
  const [open, setOpen] = useState(false)
  const initialized = useRef(false)
  const refreshTask = useRef<Promise<RefreshOutcome> | null>(null)
  function refresh() {
    refreshTask.current ??= refreshLiveUsage()
      .then((result): RefreshOutcome => {
        const failedProviders = new Set(
          result.errors
            .filter(({ detail }) => !liveFailureIsSetup(detail))
            .map(({ provider }) => provider),
        )
        const refreshed = result.providers.some(
          ({ provider, freshness }) => freshness === "fresh" && !failedProviders.has(provider),
        )
        if (failedProviders.size > 0) {
          if (!refreshed) throw new Error("Provider refresh failed")
          return "partial"
        }
        return refreshed ? "refreshed" : "checked"
      })
      .finally(() => {
        refreshTask.current = null
      })
    return refreshTask.current
  }
  const active = settings.liveUsageEnabled && settings.liveUsageStarted
  const activeLive = active ? live : null
  const pins = loaded
    ? (settings.sidebarUsagePins ?? (activeLive ? defaultUsagePins(activeLive) : []))
    : []
  const appearance = settings.sidebarUsageAppearance
  const initialize = () => {
    if (
      loaded &&
      !initialized.current &&
      settings.sidebarUsagePins === null &&
      pins.length > 0
    ) {
      initialized.current = true
      void update({ sidebarUsagePins: pins })
    }
  }
  const toggle = (pin: UsagePin) => {
    const pinned = pins.some((entry) => pinId(entry) === pinId(pin))
    noteInteraction({
      kind: "usageLimitsInteraction",
      action: pinned ? "pin_removed" : "pin_added",
    })
    void update({
      sidebarUsagePins: pinned
        ? pins.filter((entry) => pinId(entry) !== pinId(pin))
        : [...pins, pin],
    })
  }
  return (
    <>
      <SidebarUsageLimits
        live={activeLive}
        pins={pins}
        appearance={appearance}
        initialize={initialize}
        onOpen={() => {
          noteInteraction({ kind: "usageLimitsInteraction", action: "opened" })
          setOpen(true)
        }}
      />
      {open && (
        <UsageLimitsDialog
          live={live}
          pins={pins}
          toggle={toggle}
          appearance={appearance}
          changeAppearance={(next) => {
            if (next !== appearance)
              noteInteraction({
                kind: "usageLimitsInteraction",
                action: next === "dials" ? "appearance_dials" : "appearance_bars",
              })
            void update({ sidebarUsageAppearance: next })
          }}
          onRefresh={refresh}
          onEnable={enableLiveUsage}
          active={active}
          loading={!loaded || !settled}
          onClose={() => {
            noteInteraction({ kind: "usageLimitsInteraction", action: "closed" })
            setOpen(false)
          }}
        />
      )}
    </>
  )
}
function SidebarUsageLimits({
  live,
  pins,
  onOpen,
  initialize,
  appearance,
}: {
  appearance: UsageAppearance
  live: LiveUsageSummaryPayload | null
  pins: UsagePin[]
  onOpen: () => void
  initialize: () => void
}) {
  const accounts = live ? usageCandidates(live) : []
  const names = useAccountNames(accounts)
  return (
    <div className="min-w-0 py-2" ref={initialize}>
      {pins.length === 0 && (
        <button
          type="button"
          data-usage-limits-trigger
          aria-label="Pin usage limits"
          aria-haspopup="dialog"
          className="sidebar-pin-usage mx-auto flex min-h-10 min-w-10 w-fit items-center justify-center rounded-control text-label-tertiary hover:text-label-secondary"
          onClick={onOpen}
        >
          <span className="flex items-center justify-center">
            <Pin size={14} aria-hidden="true" className="shrink-0" />
            <span className="sidebar-pin-usage-label type-footnote" aria-hidden="true">
              <span className="pl-2">Pin usage limits</span>
            </span>
          </span>
        </button>
      )}
      {pins.length > 0 && (
        <div
          className={`grid max-h-40 gap-1 overflow-y-auto ${appearance === "bars" ? "grid-cols-2" : "grid-cols-3"}`}
        >
          {pins.map((pin) => {
            const account = accounts.find((entry) => entry.key === pin.account)
            const window = account?.reading.windows.find((entry) => matchesWindow(pin, entry))
            return (
              <div key={pinId(pin)} className="min-w-0">
                {account && window && live ? (
                  <ProviderRadial
                    onOpen={onOpen}
                    compact
                    showAccount={
                      accounts.filter(
                        (entry) => entry.reading.provider === account.reading.provider,
                      ).length > 1
                    }
                    appearance={appearance}
                    provider={account.reading}
                    displayName={`${names.get(account.key)}${account.reading.accountEmail ? ` · ${account.reading.accountEmail}` : ""}`}
                    selectedWindow={window}
                    status={liveProviderStatus(live, account.reading)}
                    activation={null}
                  />
                ) : (
                  <button
                    type="button"
                    onClick={onOpen}
                    data-usage-limits-trigger
                    aria-haspopup="dialog"
                    aria-label={`${pin.name}, ${pin.label}, unavailable`}
                    title={`${pin.name} · ${pin.label} · Unavailable`}
                    className="flex min-h-10 w-full items-center justify-center gap-0.5 rounded-control type-footnote text-label-tertiary hover:bg-surface-hover"
                  >
                    <UsageRing percent={null} mark={providerMark(pin.provider)} size={20} />—
                  </button>
                )}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
function UsageLimitsDialog({
  live,
  pins,
  toggle,
  onClose,
  appearance,
  changeAppearance,
  onRefresh,
  onEnable,
  active,
  loading,
}: {
  onEnable: () => Promise<void>
  active: boolean
  loading: boolean
  onRefresh: () => Promise<RefreshOutcome>
  appearance: UsageAppearance
  changeAppearance: (next: UsageAppearance) => void
  live: LiveUsageSummaryPayload | null
  pins: UsagePin[]
  toggle: (pin: UsagePin) => void
  onClose: () => void
}) {
  const refreshBusy = useRef(false)
  const [refreshing, setRefreshing] = useState(false)
  const [refreshStatus, setRefreshStatus] = useState("")
  const [failed, setFailed] = useState(false)
  async function enable() {
    if (refreshBusy.current) return
    refreshBusy.current = true
    setRefreshing(true)
    setFailed(false)
    setRefreshStatus("Enabling live usage…")
    try {
      await onEnable()
      noteInteraction({ kind: "usageLimitsInteraction", action: "enable_succeeded" })
    } catch {
      noteInteraction({ kind: "usageLimitsInteraction", action: "enable_failed" })
      setFailed(true)
      setRefreshStatus("Could not load usage limits. Try again.")
      refreshBusy.current = false
      setRefreshing(false)
      return
    }
    try {
      const outcome = await onRefresh()
      noteInteraction({ kind: "usageLimitsInteraction", action: "refresh_succeeded" })
      setRefreshStatus(
        outcome === "partial"
          ? "Live usage is on. Some limits couldn’t be refreshed."
          : "Live usage is on.",
      )
    } catch {
      noteInteraction({ kind: "usageLimitsInteraction", action: "refresh_failed" })
      setFailed(true)
      setRefreshStatus("Could not load usage limits. Try again.")
    } finally {
      refreshBusy.current = false
      setRefreshing(false)
    }
  }
  async function openUsageSettings() {
    try {
      await openSettingsWindow("usage")
      noteInteraction({ kind: "usageLimitsInteraction", action: "settings_opened" })
    } catch {
      noteInteraction({ kind: "usageLimitsInteraction", action: "settings_open_failed" })
      setRefreshStatus("Could not open Usage settings. Try again.")
    }
  }
  async function refresh() {
    if (refreshBusy.current) return
    refreshBusy.current = true
    setRefreshing(true)
    setFailed(false)
    setRefreshStatus("Refreshing usage limits…")
    try {
      const outcome = await onRefresh()
      noteInteraction({ kind: "usageLimitsInteraction", action: "refresh_succeeded" })
      setRefreshStatus(
        outcome === "partial"
          ? "Some usage limits couldn’t be refreshed."
          : outcome === "checked"
            ? "Usage limits checked."
            : "Usage limits refreshed.",
      )
    } catch {
      noteInteraction({ kind: "usageLimitsInteraction", action: "refresh_failed" })
      setFailed(true)
      setRefreshStatus("Could not refresh usage limits. Try again.")
    } finally {
      refreshBusy.current = false
      setRefreshing(false)
    }
  }
  const accounts = active && live ? usageCandidates(live) : []
  const updateFailed = failed || (live !== null && hasProviderRefreshError(live))
  const names = useAccountNames(accounts)
  const missing = pins.filter(
    (pin) =>
      !accounts.some(
        (account) =>
          account.key === pin.account &&
          account.reading.windows.some((window) => matchesWindow(pin, window)),
      ),
  )
  return (
    <ConnectionDialog
      title="Usage limits"
      initialFocus="dialog"
      scrim="modal"
      dismissOnBackdrop
      onCancel={onClose}
      getFallbackFocus={usageFocusTarget}
      headerAction={
        <div className="flex items-center">
          {active && accounts.length > 0 && (
            <Tooltip label="Refresh usage limits">
              <button
                type="button"
                aria-label="Refresh usage limits"
                aria-busy={refreshing}
                aria-disabled={refreshing}
                className={pinButtonClass}
                onClick={() => void refresh()}
              >
                <RotateCw
                  size={16}
                  aria-hidden="true"
                  className={refreshing ? "motion-safe:animate-spin" : ""}
                />
              </button>
            </Tooltip>
          )}
          <button
            type="button"
            aria-label="Close usage limits"
            className="flex size-10 shrink-0 items-center justify-center rounded-control text-label-secondary hover:bg-surface-hover"
            onClick={onClose}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </div>
      }
    >
      <div className="space-y-2 px-4 pb-4">
        <p
          role="status"
          className={refreshStatus ? "type-footnote text-label-secondary" : "sr-only"}
        >
          {refreshStatus}
        </p>
        {accounts.length > 0 && (
          <>
            <p className="type-footnote text-label-secondary">
              Pin limits to keep them in your sidebar.
            </p>
            <div className="flex flex-wrap items-center justify-between gap-3 pt-2 pb-3">
              <span className="type-body text-label">Sidebar display</span>
              <SegmentedControl
                ariaLabel="Sidebar usage display"
                options={[
                  { value: "dials", label: "Dials" },
                  { value: "bars", label: "Bars" },
                ]}
                value={appearance}
                onChange={changeAppearance}
                variant="native-tabs"
                equalWidth={false}
              />
            </div>
          </>
        )}
        {accounts.length === 0 && (
          <div
            className="flex flex-col items-center gap-3 px-4 py-6 text-center"
            aria-busy={loading || refreshing}
          >
            <span
              className="flex size-12 items-center justify-center rounded-full bg-surface-card text-label-secondary"
              aria-hidden="true"
            >
              <Gauge size={24} strokeWidth={1.5} />
            </span>
            <div className="max-w-sm space-y-2">
              <h3 className="text-balance type-title-3 text-label">
                {loading
                  ? "Checking your usage limits"
                  : !active
                    ? "See your usage limits"
                    : updateFailed
                      ? "Your limits couldn’t be updated"
                      : "Your limits will appear here"}
              </h3>
              <p className="text-pretty type-body text-label-secondary">
                {loading
                  ? "Looking for limits from your signed-in agents."
                  : !active
                    ? "Read limits from your signed-in agents and pin them here."
                    : updateFailed
                      ? "We couldn’t get a reading from your providers. Check your connection or review Usage settings."
                      : "Live usage is on. Sign in to a supported agent, such as Claude or Codex, and we’ll pick up its limits here."}
              </p>
            </div>
            {!loading && (
              <div className="flex flex-wrap items-center justify-center gap-3 pt-2">
                <PushButton
                  variant="primary"
                  className="min-h-10"
                  disabled={refreshing}
                  onClick={() => void (active ? refresh() : enable())}
                >
                  {refreshing ? "Checking…" : active ? "Check again" : "Enable live usage"}
                </PushButton>
                {active && (
                  <PushButton className="min-h-10" onClick={() => void openUsageSettings()}>
                    Usage settings
                  </PushButton>
                )}
              </div>
            )}
          </div>
        )}
        {accounts.map(({ key, reading }) => (
          <ProviderGroup
            key={key}
            provider={reading}
            displayName={names.get(key)!}
            accountCount={
              accounts.filter((entry) => entry.reading.provider === reading.provider).length
            }
            status={liveProviderStatus(live!, reading)}
            now={Date.parse(live!.generatedAt) || 0}
            activation={null}
            windowAction={(window) => {
              const pin: UsagePin = {
                account: key,
                provider: reading.provider,
                name: reading.displayName,
                window: window.id,
                scope: window.scopeModel,
                label: liveWindowLabel(window),
              }
              const pressed = pins.some((entry) => pinId(entry) === pinId(pin))
              return (
                <Tooltip label={pressed ? "Unpin from sidebar" : "Pin to sidebar"}>
                  <button
                    type="button"
                    className={pinButtonClass}
                    aria-pressed={pressed}
                    aria-label={`${pressed ? "Unpin" : "Pin"} ${reading.displayName} ${liveWindowLabel(window)}${accounts.filter((entry) => entry.reading.provider === reading.provider).length > 1 ? ` (${reading.accountEmail ?? key})` : ""}`}
                    onClick={() => toggle(pin)}
                  >
                    <Pin
                      size={14}
                      fill={pressed ? "currentColor" : "none"}
                      aria-hidden="true"
                    />
                  </button>
                </Tooltip>
              )
            }}
          />
        ))}
        {active &&
          live &&
          liveUnavailableProviders(live).map((entry) => (
            <UnavailableGroup key={entry.provider} entry={entry} />
          ))}
        {missing.map((pin) => (
          <div
            key={pinId(pin)}
            className="flex items-center justify-between gap-2 type-footnote"
          >
            <span>
              {pin.name} · {pin.label}
              <span className="text-label-secondary"> · Unavailable</span>
            </span>
            <Tooltip label="Unpin from sidebar">
              <button
                className={pinButtonClass}
                type="button"
                aria-pressed="true"
                aria-label={`Unpin ${pin.name} ${pin.label}`}
                onClick={() => toggle(pin)}
              >
                <Pin size={14} fill="currentColor" aria-hidden="true" />
              </button>
            </Tooltip>
          </div>
        ))}
      </div>
    </ConnectionDialog>
  )
}
