import { useState } from "react"

import { Card } from "../../components/ui/Card"
import { InterfaceSizeControl } from "../../components/ui/InterfaceSizeControl"
import { Pane } from "../../components/ui/Pane"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import { ToggleRow } from "../../components/ui/ToggleRow"
import type { ThemePreference } from "../../lib/ipc"
import { saveInterfaceScale, type AppSettingsController } from "./useAppSettings"
import { INTERFACE_SCALE_PRESETS, type InterfaceScaleChange } from "../../lib/interfaceScale"

const THEMES: ReadonlyArray<{ value: ThemePreference; label: string }> = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
]

/**
 * Appearance: the theme choice, and what a session's analysis opens with.
 *
 * "System" is the absence of an override, not a third palette — the token layer
 * already follows the OS when no `data-theme` is set, so choosing it removes
 * the attribute rather than writing a value.
 *
 * The Skills & MCPs switch writes the same preference the chart's own button
 * writes. It is here as well because that button sits at the foot of a long
 * table, and a reader who wants to always see everything looks in settings.
 */
export function AppearancePane({ settings, update, loaded }: AppSettingsController) {
  const [scaleError, setScaleError] = useState(false)
  const [savingScale, setSavingScale] = useState(false)
  async function changeScale(change: InterfaceScaleChange): Promise<void> {
    setSavingScale(true)
    setScaleError(false)
    try {
      await saveInterfaceScale(change)
    } catch {
      setScaleError(true)
    } finally {
      setSavingScale(false)
    }
  }
  return (
    <Pane title="Appearance">
      <SectionGroup title="Theme">
        <Card>
          <Row
            label="Appearance"
            description="System follows your operating system's light and dark setting."
            trailing={
              <SegmentedControl
                options={THEMES}
                value={settings.theme}
                ariaLabel="Appearance"
                onChange={(theme) => void update({ theme })}
              />
            }
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Interface size">
        <Card>
          <Row
            label="Interface size"
            description="Adjust text and controls across all antiburn windows. Your display scaling still applies."
          >
            <InterfaceSizeControl
              percent={settings.interfaceScalePercent}
              presets={INTERFACE_SCALE_PRESETS}
              disabled={!loaded || savingScale}
              onChange={(percent) => void changeScale({ kind: "set", percent })}
              onReset={() => void changeScale({ kind: "reset" })}
            />
            {scaleError && (
              <p role="alert" className="mt-2 type-footnote text-system-red-text">
                Could not change interface size. Try again.
              </p>
            )}
          </Row>
        </Card>
      </SectionGroup>

      <SectionGroup title="Session analysis">
        <Card>
          <ToggleRow
            label="Show every skill and MCP"
            description="A session's Skills & MCPs table lists the largest few and hides the rest behind a button. Turn this on to see the whole table every time. That button writes this setting too, so it stays wherever you last left it."
            checked={settings.skillsMcpExpanded}
            onChange={(skillsMcpExpanded) => void update({ skillsMcpExpanded })}
          />
        </Card>
      </SectionGroup>
    </Pane>
  )
}
