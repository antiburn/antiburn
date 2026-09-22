import type { ComponentProps } from "react"
import { Row } from "../../components/ui/Row"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { detectPlatform } from "../../lib/platform"
import { settingsControlLabel, type SettingsControlId } from "../../lib/settingsSearchTargets"

type SearchRowProps = { searchId: SettingsControlId; label?: string }

export function SettingsRow({
  searchId,
  label,
  ...props
}: Omit<ComponentProps<typeof Row>, "label"> & SearchRowProps) {
  return (
    <Row
      {...props}
      label={label ?? settingsControlLabel(searchId, detectPlatform())}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}

export function SettingsToggleRow({
  searchId,
  label,
  ...props
}: Omit<ComponentProps<typeof ToggleRow>, "label"> & SearchRowProps) {
  return (
    <ToggleRow
      {...props}
      label={label ?? settingsControlLabel(searchId, detectPlatform())}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}

export function SettingsSectionGroup({
  searchId,
  ...props
}: Omit<ComponentProps<typeof SectionGroup>, "title"> & { searchId: SettingsControlId }) {
  return (
    <SectionGroup
      {...props}
      title={settingsControlLabel(searchId, detectPlatform())}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}
