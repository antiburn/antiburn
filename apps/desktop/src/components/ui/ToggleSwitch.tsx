import * as Switch from "@radix-ui/react-switch"

export type ToggleSwitchProps = {
  checked: boolean
  onCheckedChange: (next: boolean) => void
  /** Required: the switch renders no visible label of its own. */
  "aria-label": string
  "aria-describedby"?: string | undefined
  /** Prevent changes without moving focus when work starts. */
  "aria-disabled"?: boolean | undefined
  /** `| undefined` so a wrapper can forward its own optional value under
   *  `exactOptionalPropertyTypes`. */
  disabled?: boolean | undefined
}

/** The standard on/off switch: a pill track with a sliding thumb.
 *
 *  A thin wrapper over the headless Radix switch, which supplies the button
 *  semantics, the checked state, and keyboard activation; the look comes
 *  entirely from `.ui-switch` / `.ui-switch-thumb` in styles/platform-controls.css. */
export function ToggleSwitch({
  checked,
  onCheckedChange,
  disabled = false,
  "aria-label": ariaLabel,
  "aria-describedby": ariaDescribedBy,
  "aria-disabled": ariaDisabled,
}: ToggleSwitchProps) {
  return (
    <Switch.Root
      className="ui-switch aria-disabled:opacity-50"
      checked={checked}
      onCheckedChange={(next) => {
        if (!ariaDisabled) onCheckedChange(next)
      }}
      aria-label={ariaLabel}
      aria-describedby={ariaDescribedBy}
      aria-disabled={ariaDisabled || undefined}
      disabled={disabled}
    >
      <Switch.Thumb className="ui-switch-thumb" />
    </Switch.Root>
  )
}
