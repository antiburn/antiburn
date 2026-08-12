// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { ReactNode } from 'react';

/** Grouped, divided, bordered container. Children are laid out as edge-to-edge
 *  rows separated by hairlines — pair with `Row` / `ToggleRow`. */
export function Card({
  children,
  className = '',
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={`divide-y divide-separator overflow-hidden rounded-popover border border-separator bg-surface-card/60 ${className}`.trimEnd()}
    >
      {children}
    </div>
  );
}
