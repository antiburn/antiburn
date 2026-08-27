import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import type { ThemePreference } from "../../lib/ipc"
import type { AppSettingsController } from "./useAppSettings"

const THEMES: ReadonlyArray<{ value: ThemePreference; label: string }> = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
]

/**
 * Appearance: the one theme choice.
 *
 * "System" is the absence of an override, not a third palette — the token layer
 * already follows the OS when no `data-theme` is set, so choosing it removes
 * the attribute rather than writing a value.
 */
export function AppearancePane({ settings, update }: AppSettingsController) {
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
    </Pane>
  )
}
