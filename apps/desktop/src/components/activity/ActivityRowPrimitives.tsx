// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useLayoutEffect, useRef, useState } from 'react';

import { relativeTime } from '../../lib/presentation/relativeTime';
import { Tooltip } from '../presentation/Tooltip';
import {
  ROW_CORNER_ATTR,
  ROW_CORNER_PINNED_ATTR,
  ROW_TITLE_SHIMMER_CLASS,
  rowTimeLabel,
  type RowTimeStatus,
} from './activityRow';

/**
 * The presentation primitives an activity row is built from.
 *
 * Value-driven and feature-blind: a row that wants to mark a problem passes an
 * icon, a tint, and `pinned`; deriving those is the caller's job. That split is
 * what lets several row kinds share one geometry without sharing each other's
 * concerns.
 */

function absoluteTime(iso: string): string | null {
  if (!iso) return null;
  const ms = new Date(iso).getTime();
  return Number.isNaN(ms) ? null : new Date(ms).toLocaleString();
}

export interface TruncatedTextProps {
  className?: string;
  text: string;
  /**
   * Sweep a highlight across the text, marking its subject as still running.
   * The text is duplicated into `data-text` for the CSS overlay to paint.
   */
  shimmer?: boolean;
}

/**
 * Single-line text that reveals its full value in a native `title` tooltip —
 * but only when it is actually cut off, so an untruncated label does not
 * sprout a redundant tooltip.
 */
export function TruncatedText({ className, text, shimmer = false }: TruncatedTextProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [truncated, setTruncated] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => setTruncated(el.scrollWidth > el.clientWidth);
    check();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(check);
    observer.observe(el);
    return () => observer.disconnect();
  }, [text]);

  return (
    <div
      ref={ref}
      className={`${className ?? ''}${shimmer ? ` ${ROW_TITLE_SHIMMER_CLASS}` : ''}`}
      title={truncated ? text : undefined}
      data-text={shimmer ? text : undefined}
      aria-label={shimmer ? text : undefined}
    >
      {text}
    </div>
  );
}

export type RowTimeCornerProps = RowTimeStatus & {
  timestamp: string;
  revealClassName?: string;
  /**
   * Render the `sr-only` copy. Pass false from a row that carries its own
   * `aria-label` — that label replaces name-from-contents, so the copy would
   * be silently dropped, and such a row must use `rowTimeLabel` itself.
   */
  announce?: boolean;
};

/**
 * A row's time, in its top-right corner.
 *
 * Positioned absolutely so it costs the row no layout width: the title runs the
 * full card while this is hidden, and revealing it on hover moves nothing.
 */
export function RowTimeCorner({
  timestamp,
  label,
  icon: Icon = null,
  tint = '',
  pinned = false,
  revealClassName = '',
  announce = true,
}: RowTimeCornerProps) {
  const absolute = absoluteTime(timestamp);

  return (
    <>
      {/* The painted corner spends most of its life inside a
          `visibility: hidden` wrapper, and that prunes it from the
          accessibility tree, not just from paint — a screen-reader user
          browsing the list would never hear when anything happened. So the
          semantics live in a permanently rendered copy here, and the corner
          below is decorative. */}
      {announce && <span className="sr-only">{rowTimeLabel(label, timestamp)}</span>}
      <div
        {...{ [ROW_CORNER_ATTR]: '' }}
        {...(pinned ? { [ROW_CORNER_PINNED_ATTR]: '' } : {})}
        className={`absolute top-2 right-2 flex items-center gap-1 ${pinned ? '' : revealClassName}`}
      >
        <Tooltip
          side="bottom"
          delayMs={150}
          label={
            <div className="space-y-0.5 text-left">
              <div className="type-caption font-medium">{label}</div>
              {absolute && <div className="type-caption text-label-tertiary">{absolute}</div>}
            </div>
          }
        >
          <span
            aria-hidden="true"
            // Muted by default — the time is supplementary next to the title,
            // and it is repeated in the tooltip and the copy above. A problem
            // outcome keeps its own tint, which must stay legible.
            className={`flex items-center gap-1 text-sm font-normal ${tint || 'text-label-tertiary'}`}
          >
            {Icon && <Icon size={11} className="shrink-0" aria-hidden="true" />}
            {relativeTime(timestamp, { compact: true })}
          </span>
        </Tooltip>
      </div>
    </>
  );
}
