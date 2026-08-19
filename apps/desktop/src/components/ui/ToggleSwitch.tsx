// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import * as Switch from "@radix-ui/react-switch"

export type ToggleSwitchProps = {
  checked: boolean
  onCheckedChange: (next: boolean) => void
  /** Required: the switch renders no visible label of its own. */
  "aria-label": string
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
}: ToggleSwitchProps) {
  return (
    <Switch.Root
      className="ui-switch"
      checked={checked}
      onCheckedChange={onCheckedChange}
      aria-label={ariaLabel}
      disabled={disabled}
    >
      <Switch.Thumb className="ui-switch-thumb" />
    </Switch.Root>
  )
}
