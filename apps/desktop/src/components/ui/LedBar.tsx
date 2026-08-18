// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { CSSProperties } from 'react';

/**
 * LED-style dot bar: a row of small circular dots instead of a solid fill,
 * per the antiburn look. `split` lists colored spans as fractions of the
 * whole (summing to at most 1); each dot takes the color of the span its
 * midpoint falls in, and anything past the last span renders as a dim slot.
 * Dots are fixed-size exact circles; the row spreads them across the full
 * width, so spacing (not dot shape) absorbs the container width.
 */
export function LedBar({
  split,
  segments = 40,
  className = '',
  blinkLast = false,
}: {
  split: Array<{ fraction: number; color: string }>;
  segments?: number;
  className?: string;
  /** Blink the last lit dot on a 1s on/off cycle — the "session live"
   *  indicator. No-op when nothing is lit. */
  blinkLast?: boolean;
}) {
  const cutoffs: Array<{ upTo: number; color: string }> = [];
  let acc = 0;
  for (const span of split) {
    acc += Math.max(0, span.fraction);
    cutoffs.push({ upTo: acc, color: span.color });
  }
  const litCount = Math.min(segments, Math.round(Math.min(1, Math.max(0, acc)) * segments));
  const blinkIndex = blinkLast && litCount > 0 ? litCount - 1 : -1;

  return (
    <div
      className={`flex w-full items-center justify-between ${className}`.trimEnd()}
      aria-hidden="true"
    >
      {Array.from({ length: segments }, (_, i) => {
        const midpoint = (i + 0.5) / segments;
        const hit = cutoffs.find((c) => midpoint <= c.upTo);
        return (
          <span
            key={i}
            className={`h-1.5 w-1.5 shrink-0 rounded-full ${hit ? '' : 'bg-surface-tertiary'} ${i === blinkIndex ? 'led-blink' : ''}`.trimEnd()}
            style={
              hit
                ? i === blinkIndex
                  ? // The blink animation alternates background-color between
                    // the lit color (via --led-on) and the dim unused-slot
                    // color, so the dot never disappears entirely.
                    ({
                      backgroundColor: hit.color,
                      '--led-on': hit.color,
                    } as CSSProperties)
                  : { backgroundColor: hit.color }
                : undefined
            }
          />
        );
      })}
    </div>
  );
}
