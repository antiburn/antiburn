import { useState } from "react"

import {
  ClaudePluginInstallDetail,
  PluginPageButton,
  claudePluginInstalledLine,
} from "../../components/claudePlugin/ClaudePluginOffer"
import { PushButton } from "../../components/ui/PushButton"
import { CLAUDE_PLUGIN_TAGLINE } from "../../lib/claudePluginOffer"
import { createClaudePluginStore, useClaudePlugin } from "../../lib/claudePluginStore"
import { SettingsRow } from "./SettingsSearchRows"

/** The Settings window's own store. It never reports `installed_detected`:
 *  the main window already counted this install. */
const settingsClaudePluginStore = createClaudePluginStore({ reportInstalled: false })

/**
 * Settings → Usage: the plugin's status and the install routes, always
 * present so search can land on it whatever the status read says.
 */
export function ClaudePluginSettingsRow() {
  const { snapshot } = useClaudePlugin(settingsClaudePluginStore, "usage_settings")
  const [open, setOpen] = useState(false)
  const status = snapshot.status
  const installed = status?.state === "installed" ? status : null
  const description = installed
    ? `${claudePluginInstalledLine(installed)} ${CLAUDE_PLUGIN_TAGLINE}`
    : status?.state === "absent"
      ? `Not installed. ${CLAUDE_PLUGIN_TAGLINE}`
      : `${CLAUDE_PLUGIN_TAGLINE} antiburn could not read Claude Code's plugin list, so it does not know whether the plugin is installed.`
  return (
    <SettingsRow
      searchId="claudePlugin"
      description={description}
      trailing={
        installed ? (
          <PluginPageButton placement="usage_settings" />
        ) : (
          <PushButton onClick={() => setOpen((previous) => !previous)}>
            {open ? "Hide" : "Install"}
          </PushButton>
        )
      }
    >
      {open && !installed && (
        <div className="col-span-2 mt-3">
          <ClaudePluginInstallDetail placement="usage_settings" />
        </div>
      )}
    </SettingsRow>
  )
}
