import type { ReactNode } from "react"

import { Tooltip } from "../presentation/Tooltip"
import { Row } from "./Row"
import { ToggleSwitch } from "./ToggleSwitch"

/** A `Row` whose trailing control is a switch. The row label doubles as the
 *  switch's accessible name, so a caller never has to repeat it. */
export function ToggleRow({
  label,
  description,
  checked,
  onChange,
  dimmed,
  disabled,
  disabledTooltip,
}: {
  label: string
  description?: string
  checked: boolean
  onChange: (next: boolean) => void
  dimmed?: boolean
  disabled?: boolean
  disabledTooltip?: ReactNode
}) {
  const toggle = (
    <ToggleSwitch
      checked={checked}
      onCheckedChange={onChange}
      aria-label={label}
      disabled={disabled}
    />
  )

  return (
    <Row
      label={label}
      description={description}
      dimmed={dimmed}
      trailing={
        disabled && disabledTooltip ? (
          <Tooltip label={disabledTooltip} side="top" delayMs={400}>
            <span
              aria-checked={checked}
              aria-disabled="true"
              aria-label={label}
              className="inline-flex min-h-10 min-w-10 items-center justify-center rounded-control"
              data-disabled-tooltip-trigger=""
              role="switch"
              tabIndex={0}
            >
              <span aria-hidden="true" className="pointer-events-none inline-flex">
                {toggle}
              </span>
            </span>
          </Tooltip>
        ) : (
          toggle
        )
      }
    />
  )
}
