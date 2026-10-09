import { SettingsToggleRow } from "./SettingsSearchRows"
import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { ToggleRow } from "../../components/ui/ToggleRow"
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
  HudVisibilitySession,
  isHudTokenMapEnabled,
  setHudTokenMapEnabled,
} from "../../lib/overlayWindow"
import { isMacOS } from "../../lib/platform"
import {
  PlanLimitsSection,
  UsageMetersSection,
  useLiveUsage,
  WorkingWeekSection,
} from "../main-window/overview/stepSettings/LimitsStepSettings"
import type { AppSettingsController } from "./useAppSettings"

/**
 * Usage: where the plan limits come from, and the one switch that turns it
 * off.
 *
 * The limits sections come from `LimitsStepSettings`, which the first-run
 * Limits card also shows. Limits has no progress-nav row, so after the first
 * run these settings live only here.
 */

export type UsagePaneProps = AppSettingsController

export function UsagePane({ settings, update }: UsagePaneProps) {
  const [hudVisibility] = useState(() => new HudVisibilitySession())
  const hudShown = useSyncExternalStore(
    hudVisibility.subscribe,
    hudVisibility.getSnapshot,
    hudVisibility.getSnapshot,
  )
  const live = useLiveUsage()
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

  return (
    <Pane title="Usage">
      <PlanLimitsSection settings={settings} update={update} />

      <WorkingWeekSection settings={settings} update={update} />

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

      <UsageMetersSection settings={settings} update={update} live={live} />
    </Pane>
  )
}
