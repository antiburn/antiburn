import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { createExternalStore } from "../../lib/externalStore"
import {
  EMPTY_LIVE_USAGE,
  getLiveUsage,
  onLiveUsageChanged,
  refreshLiveUsage,
  type LiveUsageMeterPayload,
  type LiveUsageSummaryPayload,
} from "../../lib/ipc"
import { HudVisibilitySession } from "../../lib/overlayWindow"
import { isMacOS } from "../../lib/platform"
import { liveErrorNote, liveSourceNote } from "../../lib/presentation/liveUsage"
import type { AppSettingsController } from "./useAppSettings"

/**
 * Usage: where the plan limits come from, and the one switch that turns it
 * off.
 *
 * The switch below is on by default: antiburn asks each provider directly for
 * your current usage, about every ten minutes, using the credentials your own
 * coding tools already hold, entirely over your own connection. That is
 * ordinary traffic — your own usage, from a provider you already use, with a
 * credential you already hold — so it runs without asking first, the same way
 * every other local reading in this app does. The switch exists for a reader
 * who wants none of it: turn it off and this pane has nothing to show, no
 * request is made, and no credential is read.
 *
 * The copy names both things the switch controls — it is what makes readings
 * possible at all, *and* it is what lets milestone notifications fire —
 * because a switch with two consequences has to say both or a reader turning
 * it off for one reason is surprised by the other.
 *
 * Show Meter, below, is the same switch one provider at a time. Hidden means
 * antiburn does not ask that provider, so the same two consequences apply and
 * the row says so. The list is a roster of what antiburn can meter, not a list
 * of what answered: a hidden provider reports nothing, and a list built from
 * readings would drop the row that turns it back on.
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
        void refreshLiveUsage().catch(() => undefined)
        return unlisten
      },
    }),
  )
  const live = useSyncExternalStore(store.subscribe, store.getSnapshot)

  const on = settings?.liveUsageEnabled ?? false
  const hidden = settings?.liveUsageHiddenProviders ?? []
  const meters = roster(live)

  function handleHudChange(next: boolean) {
    hudVisibility.set(next)
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
          <ToggleRow
            label="Keep my plan limits current"
            description="Asks each provider directly for your current usage, about every ten minutes, using the credentials your own coding tools already have — that's your own connection, made as you; no antiburn server is involved. When a provider can't be reached directly, antiburn falls back to asking your coding tool's own local process the same question. Turning this off also stops usage milestone notifications, since they need readings that keep moving."
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

      {isMacOS() && (
        <SectionGroup title="Floating HUD">
          <Card>
            <ToggleRow
              label="Show floating usage HUD"
              description="A small always-on-top readout of your plan limits. It expands when you hover over it, and you can drag it anywhere on screen. It shows the same figures as this pane, so it is only as current as they are — the refresh switch above is what keeps them moving."
              checked={hudShown}
              onChange={handleHudChange}
            />
          </Card>
        </SectionGroup>
      )}

      <SectionGroup title="Show Meter">
        <Card>
          {meters.map((meter) => {
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
                  name: meter.displayName,
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
              >
                {shown && reading && (
                  <p className="type-caption tabular-nums text-label-tertiary">
                    {liveSourceNote(reading)}
                  </p>
                )}
              </Row>
            )
          })}
        </Card>
      </SectionGroup>
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
  name,
}: {
  shown: boolean
  on: boolean
  reading: LiveUsageSummaryPayload["providers"][number] | undefined
  failure: LiveUsageSummaryPayload["errors"][number] | undefined
  name: string
}): string {
  if (!shown) {
    return `antiburn does not ask ${name} for usage, and ${name} milestone notifications do not fire.`
  }
  // Report what the snapshot holds before reporting a switch. A reading and a
  // failure can both be true — a stale figure that a fresh attempt could not
  // replace — and the reader needs the second sentence to read the first one
  // correctly.
  const parts: string[] = []
  if (reading) {
    parts.push(
      `${reading.sourceLabel}. ${reading.windows.length} limit${
        reading.windows.length === 1 ? "" : "s"
      } reported.`,
    )
  }
  if (failure) parts.push(liveErrorNote(failure.category))
  if (parts.length > 0) return parts.join(" ")
  if (!on) return "Turn the switch above back on to ask for current plan limits."
  return `No readings yet. Sign in with ${name} and this fills in.`
}
