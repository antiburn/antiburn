import { useState, useSyncExternalStore } from "react"

import { Card } from "../../../../components/ui/Card"
import { Row } from "../../../../components/ui/Row"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import { SegmentedControl } from "../../../../components/ui/SegmentedControl"
import { ToggleSwitch } from "../../../../components/ui/ToggleSwitch"
import { createExternalStore } from "../../../../lib/externalStore"
import {
  EMPTY_LIVE_USAGE,
  getLiveUsage,
  onLiveUsageChanged,
  refreshLiveUsage,
  startLiveUsage,
  type AppSettings,
  type LiveUsageMeterPayload,
  type LiveUsageSummaryPayload,
} from "../../../../lib/ipc"
import {
  liveDetectionNote,
  liveErrorNote,
  liveSourceAge,
  liveUnavailableReason,
  liveWindows,
} from "../../../../lib/presentation/liveUsage"
import {
  SettingsRow,
  SettingsSectionGroup,
  SettingsToggleRow,
} from "../../../settings/SettingsSearchRows"
import { useAppSettings, type AppSettingsController } from "../../../settings/useAppSettings"

type WorkingWeek = AppSettings["workingWeek"]

/** How often this section re-asks while on screen. Matches the popover. */
const USAGE_VISIBLE_POLL_MS = 60_000

/**
 * The working weeks a reader can pick. Each one starts on Monday.
 *
 * Three choices, not seven switches. A reader who works Tuesday to Saturday is
 * not served yet, and the stored value leaves room to add that later.
 */
const WORKING_WEEKS: readonly { value: WorkingWeek; label: string }[] = [
  { value: "five", label: "5 days" },
  { value: "six", label: "6 days" },
  { value: "seven", label: "7 days" },
]

/**
 * The live-usage reading, shared by the Usage pane (its old place) and this
 * step's own settings. One store per mount, cached from the last reading on
 * open, then refreshed through the shell and kept current from any window.
 */
export function useLiveUsage(): LiveUsageSummaryPayload {
  const [store] = useState(() =>
    createExternalStore({
      initial: EMPTY_LIVE_USAGE,
      load: () => getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
      subscribe: async (set) => {
        const unlisten = await onLiveUsageChanged(set)
        const refresh = () => void refreshLiveUsage().catch(() => undefined)
        refresh()
        const timer = setInterval(refresh, USAGE_VISIBLE_POLL_MS)
        return () => {
          clearInterval(timer)
          unlisten()
        }
      },
    }),
  )
  return useSyncExternalStore(store.subscribe, store.getSnapshot)
}

/** The "Keep my plan limits current" row, shared by the Usage pane (its old
 *  place) and this step's own settings. */
export function PlanLimitsSection({
  settings,
  update,
}: Pick<AppSettingsController, "settings" | "update">) {
  const on = settings?.liveUsageEnabled ?? false
  // Live usage never ran at all until a reader passed the first-run gate (or
  // skipped it, which also leaves it unstarted). The switch must read as off
  // then, even though `liveUsageEnabled` defaults to true, or a Skip in the
  // takeover would show it on with nothing behind it.
  const liveUsageStarted = settings?.liveUsageStarted ?? false
  const planLimitsOn = on && liveUsageStarted

  // Turning the switch on starts live usage first, the same gate the
  // first-run takeover uses, so the Keychain prompt it can trigger on macOS
  // only ever follows a deliberate click. A failed start leaves the switch
  // off, since it derives from `liveUsageStarted` rather than its own state.
  async function handlePlanLimitsChange(next: boolean): Promise<void> {
    if (!next) {
      await update({ liveUsageEnabled: false })
      return
    }
    if (!liveUsageStarted) {
      try {
        await startLiveUsage()
      } catch {
        return
      }
    }
    if (!on) await update({ liveUsageEnabled: true })
    void refreshLiveUsage().catch(() => undefined)
  }

  return (
    <SectionGroup title="Keeping limits current">
      <Card>
        <SettingsToggleRow
          searchId="planLimits"
          description="Asks each provider directly for your current usage every five minutes in the background, and more often while visible, using the credentials your own coding tools already have — that's your own connection, made as you; no antiburn server is involved. When a provider can't be reached directly, antiburn falls back to asking your coding tool's own local process the same question. Turning this off also stops usage milestone notifications, since they need readings that keep moving."
          checked={planLimitsOn}
          onChange={(next) => void handlePlanLimitsChange(next)}
        />
        <Row
          label="With this off"
          description="antiburn makes none of these requests and shows no plan limits at all."
        />
      </Card>
    </SectionGroup>
  )
}

/** The "Days you work" row, shared the same way. */
export function WorkingWeekSection({
  settings,
  update,
}: Pick<AppSettingsController, "settings" | "update">) {
  return (
    <SectionGroup title="Your working week">
      <Card>
        <SettingsRow
          searchId="workingWeek"
          description="Spreads your weekly allowance over these days, so a quiet weekend does not read as falling behind. Weeks start Monday. This does not change 5-hour limits."
          trailing={
            <SegmentedControl
              options={WORKING_WEEKS}
              value={settings?.workingWeek ?? "seven"}
              onChange={(next) => void update({ workingWeek: next })}
              ariaLabel="Days you work each week"
            />
          }
        />
      </Card>
    </SectionGroup>
  )
}

/** The "Track Limits for" row, shared the same way. */
export function UsageMetersSection({
  settings,
  update,
  live,
}: Pick<AppSettingsController, "settings" | "update"> & { live: LiveUsageSummaryPayload }) {
  const on = settings?.liveUsageEnabled ?? false
  const hidden = settings?.liveUsageHiddenProviders ?? []
  const meters = roster(live)

  // Write the hidden set, then refresh: a provider the reader just turned on
  // has no reading yet, and one they turned off must leave the other surfaces
  // now rather than at the next background pass.
  function handleMeterChange(provider: string, next: boolean) {
    const remaining = hidden.filter((id) => id !== provider)
    void Promise.resolve(
      update({
        liveUsageHiddenProviders: next ? remaining : [...remaining, provider],
      }),
    ).then(() => {
      void refreshLiveUsage().catch(() => undefined)
    })
  }

  return (
    <SettingsSectionGroup searchId="usageMeters">
      <p className="px-1 type-footnote text-label-secondary">
        You need to sign in inside each tool to track its limits.
      </p>
      <Card>
        {meters.map((meter) => {
          // The last reading, however old: this row reports what antiburn
          // knows, and a failed check is a reason beside it, not a reason
          // to hide it. The popover and HUD apply the grace window.
          const reading = live.providers.find(
            (provider) => provider.provider === meter.provider,
          )
          const failure = live.errors.find((error) => error.provider === meter.provider)
          const shown = !hidden.includes(meter.provider)
          return (
            <Row
              key={meter.provider}
              label={meter.displayName}
              description={meterNote({
                shown,
                on,
                reading,
                failure,
                meter,
              })}
              dimmed={!on}
              trailing={
                <ToggleSwitch
                  checked={shown}
                  onCheckedChange={(next) => handleMeterChange(meter.provider, next)}
                  aria-label={`Show ${meter.displayName} meter`}
                  disabled={!on}
                />
              }
            ></Row>
          )
        })}
      </Card>
    </SettingsSectionGroup>
  )
}

/**
 * The providers to show a switch for.
 *
 * The shell states the roster. An older cached snapshot has none, so fall back
 * to whatever the readings name — the reader keeps a working list until the
 * next refresh answers with the real one.
 */
function roster(live: LiveUsageSummaryPayload): LiveUsageMeterPayload[] {
  const named = new Map<string, LiveUsageMeterPayload>()
  if (live.meters.length > 0) {
    for (const meter of live.meters) named.set(meter.provider, meter)
  } else {
    for (const provider of live.providers) {
      named.set(provider.provider, {
        provider: provider.provider,
        displayName: provider.displayName,
        shown: true,
      })
    }
    for (const error of live.errors) {
      if (named.has(error.provider)) continue
      named.set(error.provider, {
        provider: error.provider,
        displayName: error.displayName || error.provider,
        shown: true,
      })
    }
  }
  if (!named.has("google")) {
    named.set("google", { provider: "google", displayName: "Google", shown: true })
  }
  return [...named.values()].sort((left, right) => left.provider.localeCompare(right.provider))
}

/**
 * The one line under a provider's switch.
 *
 * A hidden meter says both consequences, for the same reason the master switch
 * above does: it stops the request, and it stops that provider's milestone
 * notifications.
 */
function meterNote({
  shown,
  on,
  reading,
  failure,
  meter,
}: {
  shown: boolean
  on: boolean
  reading: LiveUsageSummaryPayload["providers"][number] | undefined
  failure: LiveUsageSummaryPayload["errors"][number] | undefined
  meter: LiveUsageMeterPayload
}): string {
  const { provider, displayName: name } = meter
  if (!shown) {
    return `antiburn does not ask ${name} for usage, and ${name} milestone notifications do not fire.`
  }
  // A reading and a failure can both be true — a figure from an earlier
  // check that the latest one could not replace — so the row keeps the
  // figure and its own check time, and adds why the latest check failed.
  if (reading) {
    const count = liveWindows(reading).length
    const line = `Signed in · ${count} limit${count === 1 ? "" : "s"} tracked ${liveSourceAge(reading)}`
    return failure
      ? `${line} · ${liveUnavailableReason(failure.category, failure.detail)}`
      : line
  }
  if (failure) {
    // A rate limit is a provider answering — the sign-in worked.
    return failure.category === "rateLimited"
      ? "Signed in · rate limited · retrying"
      : liveErrorNote(failure.category, provider, failure.detail)
  }
  return liveDetectionNote(provider, meter.detection ?? "unknown", on, meter.carrierLabel)
}

export function LimitsStepSettings() {
  const { settings, update } = useAppSettings()
  const live = useLiveUsage()

  return (
    <>
      <PlanLimitsSection settings={settings} update={update} />
      <WorkingWeekSection settings={settings} update={update} />
      <UsageMetersSection settings={settings} update={update} live={live} />
    </>
  )
}
