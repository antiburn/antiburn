// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { ReactNode } from 'react';

/** A single row inside a `Card`: label on the left, an optional trailing
 *  control on the right, and a full-width description underneath.
 *
 *  The label is `type-body` (13px/400), not the semibold `type-headline`: a card
 *  is a run of peer rows, so the label separates from its description by size
 *  and contrast (13px full-contrast over 11px secondary) the way a native
 *  settings pane does, instead of every row shouting its own heading. */
export function Row({
  label,
  description,
  trailing,
  dimmed,
  children,
}: {
  label: string;
  // `| undefined` on the forwarded props is deliberate: under
  // `exactOptionalPropertyTypes` a wrapper such as `ToggleRow` cannot pass its
  // own optional value through unless the target accepts an explicit undefined.
  description?: string | undefined;
  trailing?: ReactNode;
  dimmed?: boolean | undefined;
  /** Extra full-width content rendered below the description, e.g. a slider track. */
  children?: ReactNode;
}) {
  return (
    <div
      className={`grid min-h-[58px] grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 px-4 py-3 ${
        dimmed ? 'opacity-50' : ''
      }`}
    >
      <p className="type-body min-w-0 truncate text-label">{label}</p>
      <div className="shrink-0">{trailing}</div>
      {description && (
        <p className="type-footnote col-span-2 mt-0.5 w-full text-pretty text-label-secondary">
          {description}
        </p>
      )}
      {children && <div className="col-span-2 w-full">{children}</div>}
    </div>
  );
}
