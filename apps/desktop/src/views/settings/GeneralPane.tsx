import { SettingsToggleRow } from "./SettingsSearchRows"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { isMacOS } from "../../lib/platform"
import type { AppSettingsController } from "./useAppSettings"

export type GeneralPaneProps = AppSettingsController

/**
 * General preferences: menu bar, Dock, and login behavior.
 */
export function GeneralPane({ settings, update, loaded }: GeneralPaneProps) {
  const macOS = isMacOS()
  const trayRequired = macOS && !settings.dockIconVisible
  const dockRequired = macOS && !settings.trayIconVisible
  const trayDisabled = !loaded || trayRequired
  const dockDisabled = !loaded || dockRequired
  const recoveryDescription = "Keep one icon visible so you can reopen antiburn."
  const trayRequiredTooltip = "Turn on Show in Dock first."
  const dockRequiredTooltip = "Turn on Show in menubar first."

  return (
    <Pane title="General">
      <SectionGroup title="Application">
        <Card>
          <SettingsToggleRow
            searchId="trayIcon"
            description={
              trayRequired
                ? recoveryDescription
                : macOS
                  ? "Keep antiburn in the menu bar when the main window is closed"
                  : "When hidden, closing the main window quits antiburn."
            }
            checked={settings.trayIconVisible}
            onChange={(next) => void update({ trayIconVisible: next })}
            disabled={trayDisabled}
            dimmed={trayDisabled}
            disabledTooltip={trayRequired ? trayRequiredTooltip : undefined}
          />
          {macOS && (
            <SettingsToggleRow
              searchId="dockIcon"
              description={
                dockRequired ? recoveryDescription : "Keep antiburn available from the Dock."
              }
              checked={settings.dockIconVisible}
              onChange={(next) => void update({ dockIconVisible: next })}
              disabled={dockDisabled}
              dimmed={dockDisabled}
              disabledTooltip={dockRequired ? dockRequiredTooltip : undefined}
            />
          )}
          <SettingsToggleRow
            searchId="startAtLogin"
            description="Starts antiburn automatically at login."
            checked={settings.launchAtLogin}
            onChange={(next) => void update({ launchAtLogin: next })}
          />
        </Card>
      </SectionGroup>
    </Pane>
  )
}
