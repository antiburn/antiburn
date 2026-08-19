// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronDown } from 'lucide-react';
import { useId, useState, type ReactNode } from 'react';

/** Horizontal inset for a disclosure's header and body.
 *
 *  `surface` sits on the bare window at the same 4px inset as a section
 *  header, for prose that would read as an over-built list if every paragraph
 *  were given a container. `card` matches `Row`'s 16px, so a group dropped
 *  inside a `Card` lines up with the card rows above it. */
export type DisclosureInset = 'surface' | 'card';

const INSET = { surface: 'px-1', card: 'px-4' } as const;

/**
 * A single collapsible disclosure: a full-width header button that reveals its
 * body, hairline-separated from its neighbours.
 *
 * Uncontrolled, because a disclosure's open state is presentation, not data.
 * Pass `defaultOpen` for the one that should start expanded.
 *
 * Hand-rolled rather than pulled from a library: the whole contract here is a
 * button, `aria-expanded`, and `aria-controls`, which is not worth a
 * dependency.
 */
export function Disclosure({
  label,
  children,
  defaultOpen = false,
  inset = 'surface',
  className = '',
}: {
  label: string;
  children: ReactNode;
  defaultOpen?: boolean;
  inset?: DisclosureInset;
  className?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();

  return (
    <div className={`border-b border-separator last:border-b-0 ${className}`.trimEnd()}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((v) => !v)}
        // No hover fill and no press feedback: these sit on the bare window
        // surface as prose, not as list rows, and the global button:active
        // scale/opacity rule (styles/controls.css) made a paragraph heading
        // twitch. The chevron rotation is the affordance.
        // `active:transform-none` is what actually cancels that rule —
        // Tailwind's `scale-*` sets the separate `scale` property and composes
        // with its `transform` instead.
        className={`flex w-full items-center justify-between gap-3 rounded-control py-3 text-left active:transform-none active:opacity-100 ${INSET[inset]}`}
      >
        <span className="type-body text-label">{label}</span>
        <ChevronDown
          size={14}
          strokeWidth={2}
          aria-hidden="true"
          className={`shrink-0 text-label-secondary transition-transform duration-[120ms] ease-out ${
            open ? 'rotate-180' : ''
          }`.trimEnd()}
        />
      </button>
      {/* Unmounted rather than hidden when collapsed, so collapsed prose stays
          out of the accessibility tree and out of find-in-page. */}
      {open && (
        <div
          id={bodyId}
          className={`type-body pb-3 text-pretty text-label-secondary ${INSET[inset]}`}
        >
          {children}
        </div>
      )}
    </div>
  );
}

/** A hairline-separated stack of `Disclosure`s.
 *
 *  On the bare surface the group draws its own top hairline, since nothing
 *  else marks where it starts. Inside a `Card` it must not: the card's own
 *  border is already that line, and a second one lands directly on it. */
export function DisclosureGroup({
  children,
  inset = 'surface',
  className = '',
}: {
  children: ReactNode;
  inset?: DisclosureInset;
  className?: string;
}) {
  return (
    <div
      className={`${inset === 'surface' ? 'border-t border-separator' : ''} ${className}`.trim()}
    >
      {children}
    </div>
  );
}
