import { useId, type ComponentProps, type ReactNode } from "react"
import { Row } from "../../components/ui/Row"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { detectPlatform } from "../../lib/platform"
import { settingsControlLabel, type SettingsControlId } from "../../lib/settingsSearchTargets"

type SearchRowProps = { searchId: SettingsControlId; label?: string }

export function SettingsField({
  searchId,
  children,
}: {
  searchId: SettingsControlId
  children: (id: string) => ReactNode
}) {
  const id = useId()
  return (
    <div data-settings-control={searchId} tabIndex={-1} className="min-w-0 space-y-2">
      <label htmlFor={id} className="block type-body text-label">
        {settingsControlLabel(searchId, detectPlatform())}
      </label>
      {children(id)}
    </div>
  )
}

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
  hideTitle = false,
  searchId,
  ...props
}: Omit<ComponentProps<typeof SectionGroup>, "title"> & {
  hideTitle?: boolean
  searchId: SettingsControlId
}) {
  return (
    <SectionGroup
      {...props}
      {...(hideTitle ? {} : { title: settingsControlLabel(searchId, detectPlatform()) })}
      data-settings-control={searchId}
      tabIndex={-1}
    />
  )
}
