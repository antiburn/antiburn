// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Terminal } from 'lucide-react';
import type { ReactNode } from 'react';

import { Tooltip } from './Tooltip';

export interface WslOriginBadgeProps {
  /** WSL distribution the session or clone was found in. */
  distro?: string | null | undefined;
  /**
   * Glyph painted inside the badge. Defaults to a neutral terminal mark; pass
   * a slot to use whatever artwork the surrounding app ships.
   */
  icon?: ReactNode | undefined;
}

/**
 * A compact mark that a session or repository lives inside a WSL distribution
 * rather than on the Windows filesystem.
 *
 * Deliberately tiny: it sits inline beside a repository chip, where a written
 * "Ubuntu-24.04 (WSL)" would crowd out the name it is qualifying. The full
 * sentence is carried by the accessible name and the tooltip, so nothing is
 * lost to the abbreviation — the glyph itself is decorative.
 */
export function WslOriginBadge({ distro, icon }: WslOriginBadgeProps) {
  if (!distro) return null;
  const label = `Found in ${distro} on Windows Subsystem for Linux`;
  return (
    <Tooltip label={label}>
      <span
        aria-label={label}
        className="inline-flex h-[15px] w-[16px] shrink-0 cursor-default items-center justify-center rounded bg-label/[0.06] text-label-secondary"
      >
        {icon ?? <Terminal size={11} aria-hidden="true" />}
      </span>
    </Tooltip>
  );
}
