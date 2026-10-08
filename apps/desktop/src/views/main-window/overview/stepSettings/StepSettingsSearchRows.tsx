import type { ComponentProps } from "react"
import { Row } from "../../../../components/ui/Row"
import { ToggleRow } from "../../../../components/ui/ToggleRow"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import {
  stepSettingsControlLabel,
  type StepSettingsControlId,
} from "../../../../lib/stepSettingsTargets"

/**
 * The step-settings equivalent of `views/settings/SettingsSearchRows.tsx`,
 * for a control a progress step's modal owns rather than a Settings pane.
 * Same `data-settings-control` attribute, so `SettingsTargetFocus` reveals
 * either kind of row without caring which one it is.
 */
type SearchRowProps = { searchId: StepSettingsControlId; label?: string }

export function StepSettingsRow({
  searchId,
  label,
  ...props
}: Omit<ComponentProps<typeof Row>, "label"> & SearchRowProps) {
  return (
    <Row
      {...props}
      label={label ?? stepSettingsControlLabel(searchId)}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}

export function StepSettingsToggleRow({
  searchId,
  label,
  ...props
}: Omit<ComponentProps<typeof ToggleRow>, "label"> & SearchRowProps) {
  return (
    <ToggleRow
      {...props}
      label={label ?? stepSettingsControlLabel(searchId)}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}

export function StepSettingsSectionGroup({
  hideTitle = false,
  searchId,
  ...props
}: Omit<ComponentProps<typeof SectionGroup>, "title"> & {
  hideTitle?: boolean
  searchId: StepSettingsControlId
}) {
  return (
    <SectionGroup
      {...props}
      {...(hideTitle ? {} : { title: stepSettingsControlLabel(searchId) })}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}
