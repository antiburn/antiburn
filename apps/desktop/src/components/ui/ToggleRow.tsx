// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Row } from './Row';
import { ToggleSwitch } from './ToggleSwitch';

/** A `Row` whose trailing control is a switch. The row label doubles as the
 *  switch's accessible name, so a caller never has to repeat it. */
export function ToggleRow({
  label,
  description,
  checked,
  onChange,
  dimmed,
  disabled,
}: {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  dimmed?: boolean;
  disabled?: boolean;
}) {
  return (
    <Row
      label={label}
      description={description}
      dimmed={dimmed}
      trailing={
        <ToggleSwitch
          checked={checked}
          onCheckedChange={onChange}
          aria-label={label}
          disabled={disabled}
        />
      }
    />
  );
}
