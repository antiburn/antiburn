import { SettingsRow, SettingsSectionGroup, SettingsToggleRow } from "./SettingsSearchRows"
import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { createExternalStore } from "../../lib/externalStore"
import {
  getHudIslandState,
  HUD_ISLAND_OFF,
  isHudIslandAvailable,
  onHudIslandState,
  setHudIsland,
  type HudIslandState,
} from "../../lib/hudIsland"
import {
  EMPTY_LIVE_USAGE,
  getLiveUsage,
  onLiveUsageChanged,
  refreshLiveUsage,
  type AppSettings,
  type LiveUsageMeterPayload,
  type LiveUsageSummaryPayload,
} from "../../lib/ipc"
import {
  HudVisibilitySession,
  isHudTokenMapEnabled,
  setHudTokenMapEnabled,
} from "../../lib/overlayWindow"
import { isMacOS } from "../../lib/platform"
import {
  liveDetectionNote,
  liveErrorNote,
  liveSourceAge,
  liveUnavailableReason,
  liveWindows,
} from "../../lib/presentation/liveUsage"
import type { AppSettingsController } from "./useAppSettings"

type WorkingWeek = AppSettings["workingWeek"]

/** How often the pane re-asks while on screen. Matches the popover. */
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
 * Usage: where the plan limits come from, and the one switch that turns it
 * off.
 *
 * The switch below is on by default. antiburn asks each provider directly for
 * current usage every five minutes in the background, and more often while
 * visible. It uses the credentials your own coding tools already hold. The
 * requests use your own connection. This traffic reads your usage from a
 * provider you already use. The switch lets you stop all requests and
 * credential reads.
 *
 * The copy names both things the switch controls — it is what makes readings
 * possible at all, *and* it is what lets milestone notifications fire —
 * because a switch with two consequences has to say both or a reader turning
 * it off for one reason is surprised by the other.
 *
 * The provider switches apply the same controls to one provider at a time.
 * Hidden providers produce no readings. The roster keeps their switches available.
 */

export type UsagePaneProps = AppSettingsController

export function UsagePane({ settings, update }: UsagePaneProps) {
  const [hudVisibility] = useState(() => new HudVisibilitySession())
  const hudShown = useSyncExternalStore(
    hudVisibility.subscribe,
    hudVisibility.getSnapshot,
    hudVisibility.getSnapshot,
  )
  // Show the cached value on open. Then refresh through the shell and accept
  // updates from this window or the popover.
  const [store] = useState(() =>
    createExternalStore({
      initial: EMPTY_LIVE_USAGE,
      load: () => getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
      subscribe: async (set) => {
        const unlisten = await onLiveUsageChanged(set)
        // Ask now, then keep asking while this pane is on screen — the same
        // cadence as the open popover — so a reader who signs in inside a
        // tool sees the row change without leaving Settings. The backend's
        // cooldown decides whether a poll reaches the network; detection
        // runs on every one.
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
  const live = useSyncExternalStore(store.subscribe, store.getSnapshot)
  // The island switch shows only on a Mac with a notch. The shell owns the
  // state: the HUD's own drag can put it in the notch or take it out.
  const [islandStore] = useState(() =>
    createExternalStore<{ available: boolean; state: HudIslandState }>({
      initial: { available: false, state: HUD_ISLAND_OFF },
      load: async () => {
        const available = await isHudIslandAvailable().catch(() => false)
        const state = available
          ? await getHudIslandState().catch(() => HUD_ISLAND_OFF)
          : HUD_ISLAND_OFF
        return { available, state }
      },
      subscribe: (set) =>
        onHudIslandState(async (state) => {
          // A state that is not "off" proves a notch. "off" does not, so ask.
          const available =
            state.island !== "off" || (await isHudIslandAvailable().catch(() => false))
          set({ available, state })
        }),
    }),
  )
  const island = useSyncExternalStore(islandStore.subscribe, islandStore.getSnapshot)
  const on = settings?.liveUsageEnabled ?? false
  const hidden = settings?.liveUsageHiddenProviders ?? []
  const meters = roster(live)

  const [tokenMapShown, setTokenMapShown] = useState(isHudTokenMapEnabled)
  function handleTokenMapChange(next: boolean) {
    setTokenMapShown(setHudTokenMapEnabled(next))
  }

  function handleHudChange(next: boolean) {
    hudVisibility.set(next)
  }

  const inNotch = island.state.island !== "off"
  // The docking row carries the notch button, so the row says why it is off.
  const islandNote = !island.available
    ? " The notch is not an option right now: no display with a notch is in use."
    : inNotch
      ? " The HUD is in the notch now, black on black, with the live light on one side and the spend rate on the other. Rest the pointer on the notch to open it, or drag the HUD out to bring it back."
      : " You can also move the HUD into the notch of the built-in display, black on black, with the live light on one side and the spend rate on the other. Dragging the HUD onto the notch does the same."

  function handleIslandChange(next: boolean) {
    void setHudIsland(next)
      .then(() => islandStore.refresh())
      .catch(() => undefined)
  }

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
    <Pane title="Usage">
      <SectionGroup title="Keeping limits current">
        <Card>
          <SettingsToggleRow
            searchId="planLimits"
            description="Asks each provider directly for your current usage every five minutes in the background, and more often while visible, using the credentials your own coding tools already have — that's your own connection, made as you; no antiburn server is involved. When a provider can't be reached directly, antiburn falls back to asking your coding tool's own local process the same question. Turning this off also stops usage milestone notifications, since they need readings that keep moving."
            checked={on}
            onChange={(next) =>
              void Promise.resolve(update({ liveUsageEnabled: next })).then(() => {
                void refreshLiveUsage().catch(() => undefined)
              })
            }
          />
          <Row
            label="With this off"
            description="antiburn makes none of these requests and shows no plan limits at all."
          />
        </Card>
      </SectionGroup>

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

      {isMacOS() && (
        <SectionGroup title="Floating HUD">
          <Card>
            <SettingsToggleRow
              searchId="floatingHud"
              description="A small always-on-top readout of your plan limits. It expands when you hover over it, and you can drag it anywhere on screen. It shows the same figures as this pane, so it is only as current as they are — the refresh switch above is what keeps them moving."
              checked={hudShown}
              onChange={handleHudChange}
            />
            <ToggleRow
              label="Show what live sessions are doing"
              description="A small map above the bars: one blob per session that wrote in the last 90 seconds. Each dot stands for a set number of tokens a minute, from 250 up, and takes the colour of the kind of work (looking, running, changing, delegating, thinking, talking)."
              checked={tokenMapShown}
              onChange={handleTokenMapChange}
            />
            <Row
              label="Docking"
              description={`Drag the HUD against any edge of the screen to dock it there. A small tab stays visible; rest the pointer on it to peek the HUD in. Drag it away to undock. A docked HUD also peeks in when a session starts writing after an hour of quiet, or when spend runs hot.${islandNote}`}
              trailing={
                <PushButton
                  onClick={() => handleIslandChange(!inNotch)}
                  disabled={!hudShown || !island.available}
                >
                  {inNotch ? "Move out of Notch" : "Move to Notch"}
                </PushButton>
              }
            />
          </Card>
        </SectionGroup>
      )}

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
    </Pane>
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
